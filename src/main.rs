mod ascii;
mod camera;
mod cli;
mod color;
mod config;
mod menu;
mod names;
mod record;
mod render;
mod screenshot;
mod style;
mod vcam;

use anyhow::{anyhow, Result};
use crossterm::{
    cursor,
    event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::fmt::Write as FmtWrite;
use std::io::{stdin, stdout, BufRead, Write as IoWrite};
use std::time::{Duration, Instant};

const TARGET_FPS: u32 = 30;
const STATUS_TIME: Duration = Duration::from_secs(3);
/// Settings are written at most this often while you fiddle with them.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const KEY_HINT: &str =
    "Esc menu · ←→ style · m mode · r record · v virtual cam · Space screenshot · q quit";

fn main() -> Result<()> {
    let args = cli::parse(std::env::args().skip(1))?;
    match args.command {
        cli::Command::Help => {
            print!("{}", cli::HELP);
            return Ok(());
        }
        cli::Command::Version => {
            println!("terminal-cam {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Command::ListStyles => {
            print!("{}", cli::styles_listing());
            return Ok(());
        }
        cli::Command::ListCameras | cli::Command::Run => {}
    }

    let cameras = camera::list_cameras()?;
    if args.command == cli::Command::ListCameras {
        for (i, cam) in cameras.iter().enumerate() {
            println!("  [{}] {}", i + 1, cam.name);
        }
        return Ok(());
    }
    if cameras.is_empty() {
        return Err(anyhow!("no cameras found"));
    }

    let mut app = App::new(args, cameras)?;

    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        Clear(ClearType::All),
        cursor::Hide
    )?;

    let result = app.run();
    let farewell = app.shutdown();

    execute!(
        stdout(),
        cursor::Show,
        Clear(ClearType::All),
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;

    for line in farewell {
        println!("{line}");
    }
    result
}

/// Picks the camera: `--camera` (number or name fragment) wins, then the
/// one used last time, then the only camera, then an interactive prompt.
/// Returns the index and whether the user was prompted (so it gets saved).
fn choose_camera(
    cameras: &[camera::CameraInfo],
    requested: Option<&str>,
    saved: Option<&str>,
) -> Result<(u32, bool)> {
    if let Some(req) = requested {
        if let Ok(n) = req.parse::<usize>() {
            return cameras
                .get(n.wrapping_sub(1))
                .map(|c| (c.index, false))
                .ok_or_else(|| anyhow!("--camera {n} out of range 1..={}", cameras.len()));
        }
        let needle = req.to_lowercase();
        return cameras
            .iter()
            .find(|c| c.name.to_lowercase().contains(&needle))
            .map(|c| (c.index, false))
            .ok_or_else(|| anyhow!("no camera matching {req:?} (try --list-cameras)"));
    }
    if let Some(cam) = saved.and_then(|name| cameras.iter().find(|c| c.name == name)) {
        return Ok((cam.index, false));
    }
    if cameras.len() == 1 {
        return Ok((cameras[0].index, false));
    }
    prompt_camera(cameras).map(|idx| (idx, true))
}

fn prompt_camera(cameras: &[camera::CameraInfo]) -> Result<u32> {
    let mut out = stdout().lock();
    writeln!(out, "Multiple cameras detected:")?;
    for (i, cam) in cameras.iter().enumerate() {
        writeln!(out, "  [{}] {}", i + 1, cam.name)?;
    }
    write!(out, "Select camera [1-{}, default 1]: ", cameras.len())?;
    out.flush()?;
    drop(out);

    let mut line = String::new();
    stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(cameras[0].index);
    }
    let choice: usize = trimmed
        .parse()
        .map_err(|_| anyhow!("invalid selection: {trimmed}"))?;
    if choice < 1 || choice > cameras.len() {
        return Err(anyhow!(
            "selection {choice} out of range 1..={}",
            cameras.len()
        ));
    }
    Ok(cameras[choice - 1].index)
}

struct App {
    cameras: Vec<camera::CameraInfo>,
    capture: camera::CaptureHandle,
    current_camera: u32,
    cfg: render::RenderConfig,
    menu: menu::MenuState,
    in_menu: bool,
    /// Live settings; `saved` is what's on disk, compared once a second.
    settings: config::AppConfig,
    saved: config::AppConfig,
    last_save_check: Instant,
    virtual_cam: Option<vcam::VirtualCam>,
    vcam_detail: vcam::VcamDetail,
    recorder: Option<record::Recorder>,
    finishing: Vec<record::Finishing>,
    status: Option<StatusMessage>,
    start: Instant,
}

enum Flow {
    Continue,
    Quit,
}

impl App {
    fn new(args: cli::Args, cameras: Vec<camera::CameraInfo>) -> Result<Self> {
        let mut settings = config::AppConfig::load()?;
        let mut cfg = render::RenderConfig {
            detected: color::detect(),
            ..Default::default()
        };
        settings.apply_to(&mut cfg);

        // Flags override saved settings for this run only.
        if let Some(v) = args.style {
            cfg.style = v;
        }
        if let Some(v) = args.mode {
            cfg.mode = v;
        }
        if let Some(v) = args.depth {
            cfg.depth = v;
        }
        if let Some(v) = args.mirror {
            cfg.mirror = v;
        }
        if let Some(v) = args.brightness {
            cfg.brightness = v;
        }
        if let Some(v) = args.contrast {
            cfg.contrast = v;
        }
        let vcam_detail = args.vcam_detail.unwrap_or(settings.vcam_detail);
        let vcam_on = args.virtual_cam.unwrap_or(settings.virtual_cam);

        let (current_camera, prompted) =
            choose_camera(&cameras, args.camera.as_deref(), settings.camera.as_deref())?;
        let capture = camera::spawn_capture(current_camera)?;

        settings.capture_from(&cfg);
        settings.virtual_cam = vcam_on;
        settings.vcam_detail = vcam_detail;
        // Baseline includes flag overrides, so a one-off `--style` isn't
        // written unless you also change something in the app.
        let saved = settings.clone();
        if prompted {
            settings.camera = cameras
                .iter()
                .find(|c| c.index == current_camera)
                .map(|c| c.name.clone());
        }

        let menu = menu::MenuState::new(
            cameras.clone(),
            current_camera,
            settings.screenshot_dir.clone(),
        );

        let mut status = None;
        set_status_for(&mut status, KEY_HINT.to_string(), Duration::from_secs(5));

        Ok(Self {
            cameras,
            capture,
            current_camera,
            cfg,
            menu,
            in_menu: false,
            settings,
            saved,
            last_save_check: Instant::now(),
            virtual_cam: vcam_on.then(vcam::VirtualCam::new),
            vcam_detail,
            recorder: None,
            finishing: Vec::new(),
            status,
            start: Instant::now(),
        })
    }

    fn run(&mut self) -> Result<()> {
        let frame_budget = Duration::from_micros(1_000_000 / TARGET_FPS as u64);
        let mut scratch = String::with_capacity(1 << 18);
        let mut render_state = render::RenderState::new();

        loop {
            let loop_start = Instant::now();
            let time = self.start.elapsed().as_secs_f32();
            let (cols, rows) = size().unwrap_or((80, 24));

            while poll(Duration::from_millis(0))? {
                if let Event::Key(k) = read()? {
                    if k.kind == KeyEventKind::Press {
                        if let Flow::Quit = self.on_key(k, cols, rows, time)? {
                            return Ok(());
                        }
                    }
                }
            }

            self.housekeeping();

            let frame_opt = self.capture.frame.lock().clone();
            if let Some(frame) = frame_opt {
                self.feed_outputs(&frame, time);

                let mask = self.in_menu.then(|| menu::bounds(&self.menu, cols, rows));
                let view = render::Viewport { cols, rows, mask };
                render::render(
                    &frame,
                    view,
                    &self.cfg,
                    time,
                    &mut render_state,
                    &mut scratch,
                );
                if self.in_menu {
                    self.sync_menu();
                    menu::draw(&self.menu, &self.cfg, cols, rows, &mut scratch);
                }
                if let Some(rec) = &self.recorder {
                    draw_rec_badge(rec.elapsed(), cols, &mut scratch);
                }
                if let Some(message) = &self.status {
                    draw_status(message, cols, rows, &mut scratch);
                }
                render::flush(&scratch)?;
            } else {
                let msg = self
                    .capture
                    .error
                    .lock()
                    .clone()
                    .unwrap_or_else(|| "waiting for camera…".to_string());
                let mut o = stdout().lock();
                write!(o, "\x1b[H\x1b[0m{msg}\x1b[0K")?;
                o.flush()?;
            }

            let elapsed = loop_start.elapsed();
            if elapsed < frame_budget {
                std::thread::sleep(frame_budget - elapsed);
            }
        }
    }

    fn on_key(&mut self, k: KeyEvent, cols: u16, rows: u16, time: f32) -> Result<Flow> {
        let typing = self.menu.is_editing_text();
        if !typing {
            match k.code {
                KeyCode::Char(' ') => {
                    self.screenshot(cols, rows, time);
                    return Ok(Flow::Continue);
                }
                KeyCode::Char('r') => {
                    self.toggle_recording(cols, rows);
                    return Ok(Flow::Continue);
                }
                _ => {}
            }
        }

        if self.in_menu {
            let action = self.menu.on_key(k);
            return self.on_menu_action(action);
        }

        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                self.in_menu = true;
                self.menu.current_camera = self.current_camera;
            }
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                return Ok(Flow::Quit)
            }
            (KeyCode::Left, _) | (KeyCode::Right, _) => {
                let dir = if k.code == KeyCode::Left { -1 } else { 1 };
                self.cfg.style = self.cfg.style.cycle(dir);
                self.say(format!("style: {}", self.cfg.style.label()));
            }
            (KeyCode::Char('m'), _) => {
                self.cfg.mode = self.cfg.mode.cycle(1);
                self.say(format!("render mode: {}", self.cfg.mode.label()));
            }
            (KeyCode::Char('v'), _) => self.toggle_virtual_cam(),
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn on_menu_action(&mut self, action: menu::Action) -> Result<Flow> {
        match action {
            menu::Action::None => {}
            menu::Action::Close => self.in_menu = false,
            menu::Action::Quit => return Ok(Flow::Quit),
            menu::Action::SwitchCamera(idx) => {
                if idx != self.current_camera {
                    self.capture.stop();
                    self.capture = camera::spawn_capture(idx)?;
                    self.current_camera = idx;
                    self.settings.camera = self
                        .cameras
                        .iter()
                        .find(|c| c.index == idx)
                        .map(|c| c.name.clone());
                }
            }
            menu::Action::CycleStyle(dir) => self.cfg.style = self.cfg.style.cycle(dir),
            menu::Action::CycleMode(dir) => self.cfg.mode = self.cfg.mode.cycle(dir),
            menu::Action::CycleDepth(dir) => self.cfg.depth = self.cfg.depth.cycle(dir),
            menu::Action::ToggleMirror => self.cfg.mirror = !self.cfg.mirror,
            menu::Action::AdjustBrightness(d) => {
                self.cfg.brightness = (self.cfg.brightness + d).clamp(-1.0, 1.0);
            }
            menu::Action::AdjustContrast(d) => {
                self.cfg.contrast = (self.cfg.contrast + d).clamp(0.1, 3.0);
            }
            menu::Action::ToggleVirtualCam => self.toggle_virtual_cam(),
            menu::Action::CycleVcamDetail(dir) => self.vcam_detail = self.vcam_detail.cycle(dir),
            menu::Action::SetScreenshotDir(input) => {
                match self.settings.set_screenshot_dir_from_input(&input) {
                    Ok(()) => {
                        self.menu
                            .set_screenshot_dir(self.settings.screenshot_dir.clone());
                        let text = match self.save_settings() {
                            Ok(()) => format!(
                                "screenshot path saved: {}",
                                self.settings.screenshot_dir.display()
                            ),
                            Err(err) => format!("screenshot path failed: {err:#}"),
                        };
                        self.say(text);
                    }
                    Err(err) => self.say(format!("screenshot path failed: {err:#}")),
                }
            }
        }
        Ok(Flow::Continue)
    }

    fn screenshot(&mut self, cols: u16, rows: u16, time: f32) {
        let frame_opt = self.capture.frame.lock().clone();
        let text = match frame_opt {
            Some(frame) => match screenshot::save(
                &frame,
                cols,
                rows,
                &self.cfg,
                time,
                &self.settings.screenshot_dir,
            ) {
                Ok(path) => format!("screenshot saved: {}", path.display()),
                Err(err) => format!("screenshot failed: {err:#}"),
            },
            None => "screenshot failed: no camera frame yet".to_string(),
        };
        self.say(text);
    }

    fn toggle_recording(&mut self, cols: u16, rows: u16) {
        if let Some(rec) = self.recorder.take() {
            self.finishing.push(rec.stop());
            self.say("recording stopped — saving GIF…".to_string());
            return;
        }
        match record::Recorder::start(&self.settings.screenshot_dir, cols, rows) {
            Ok(rec) => {
                self.recorder = Some(rec);
                self.say("recording GIF — press r to stop".to_string());
            }
            Err(err) => self.say(format!("recording failed: {err:#}")),
        }
    }

    fn toggle_virtual_cam(&mut self) {
        if self.virtual_cam.take().is_some() {
            self.say("virtual camera: off".to_string());
        } else {
            self.virtual_cam = Some(vcam::VirtualCam::new());
            self.say("virtual camera: on".to_string());
        }
    }

    /// Pushes the frame to the virtual camera and the GIF recorder.
    fn feed_outputs(&mut self, frame: &camera::Frame, time: f32) {
        if let Some(vc) = self.virtual_cam.as_mut() {
            let text = match vc.push(frame, &self.cfg, self.vcam_detail, time) {
                vcam::VcamEvent::Connected => {
                    Some("virtual camera: connected — select “Terminal Cam” in your video app")
                }
                vcam::VcamEvent::Waiting => {
                    Some("virtual camera: device not found — install it via TerminalCamCamera.app")
                }
                vcam::VcamEvent::Disconnected => Some("virtual camera: connection lost, retrying…"),
                vcam::VcamEvent::Unsupported => Some("virtual camera: only available on macOS"),
                vcam::VcamEvent::None => None,
            };
            if let Some(text) = text {
                self.say(text.to_string());
            }
        }

        let keep_recording = self
            .recorder
            .as_mut()
            .is_none_or(|rec| rec.tick(frame, &self.cfg, time));
        if !keep_recording {
            if let Some(rec) = self.recorder.take() {
                self.finishing.push(rec.stop());
                self.say(format!(
                    "recording hit the {} s limit — saving GIF…",
                    record::MAX_DURATION.as_secs()
                ));
            }
        }
    }

    /// Expires the status line, reports finished GIFs, and saves changed
    /// settings (at most once per `SAVE_INTERVAL`).
    fn housekeeping(&mut self) {
        if self
            .status
            .as_ref()
            .is_some_and(|message| Instant::now() >= message.until)
        {
            self.status = None;
        }

        let mut i = 0;
        while i < self.finishing.len() {
            if self.finishing[i].is_done() {
                let text = describe_saved(self.finishing.swap_remove(i).wait());
                self.say(text);
            } else {
                i += 1;
            }
        }

        if self.last_save_check.elapsed() >= SAVE_INTERVAL {
            self.last_save_check = Instant::now();
            if let Err(err) = self.save_settings() {
                self.say(format!("could not save settings: {err:#}"));
            }
        }
    }

    /// Writes settings if they changed since the last write. A failed
    /// write is reported once, not retried every second.
    fn save_settings(&mut self) -> Result<()> {
        self.settings.capture_from(&self.cfg);
        self.settings.virtual_cam = self.virtual_cam.is_some();
        self.settings.vcam_detail = self.vcam_detail;
        if self.settings == self.saved {
            return Ok(());
        }
        self.saved = self.settings.clone();
        self.settings.save()
    }

    fn sync_menu(&mut self) {
        self.menu.vcam_on = self.virtual_cam.is_some();
        self.menu.vcam_status = self
            .virtual_cam
            .as_ref()
            .map(|vc| vc.status_label())
            .unwrap_or("");
        self.menu.vcam_detail = self.vcam_detail;
    }

    /// Stops recording, waits for GIFs to finish, saves settings. Returns
    /// lines to print once the terminal is restored.
    fn shutdown(&mut self) -> Vec<String> {
        if let Some(rec) = self.recorder.take() {
            self.finishing.push(rec.stop());
        }
        let mut lines: Vec<String> = self
            .finishing
            .drain(..)
            .map(|f| describe_saved(f.wait()))
            .collect();
        if let Err(err) = self.save_settings() {
            lines.push(format!("could not save settings: {err:#}"));
        }
        lines
    }

    fn say(&mut self, text: String) {
        set_status_for(&mut self.status, text, STATUS_TIME);
    }
}

fn describe_saved(result: Result<record::Saved>) -> String {
    match result {
        Ok(saved) => format!(
            "recording saved: {} ({} frames)",
            saved.path.display(),
            saved.frames
        ),
        Err(err) => format!("recording failed: {err:#}"),
    }
}

struct StatusMessage {
    text: String,
    until: Instant,
}

fn set_status_for(status: &mut Option<StatusMessage>, text: String, duration: Duration) {
    *status = Some(StatusMessage {
        text,
        until: Instant::now() + duration,
    });
}

fn draw_status(status: &StatusMessage, cols: u16, rows: u16, buf: &mut String) {
    if cols == 0 || rows == 0 {
        return;
    }

    let width = cols as usize;
    let text: String = status.text.chars().take(width).collect();
    let pad = width.saturating_sub(text.chars().count());
    let _ = write!(
        buf,
        "\x1b[{};1H\x1b[7m{}{}\x1b[0m",
        rows,
        text,
        " ".repeat(pad)
    );
}

/// Red "● REC m:ss" badge in the top-right corner while recording.
fn draw_rec_badge(elapsed: Duration, cols: u16, buf: &mut String) {
    let secs = elapsed.as_secs();
    let badge = format!(" ● REC {}:{:02} ", secs / 60, secs % 60);
    let width = badge.chars().count() as u16;
    if cols < width {
        return;
    }
    let _ = write!(
        buf,
        "\x1b[1;{}H\x1b[41;97;1m{}\x1b[0m",
        cols - width + 1,
        badge
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cams() -> Vec<camera::CameraInfo> {
        ["FaceTime HD Camera", "Logitech BRIO"]
            .iter()
            .enumerate()
            .map(|(i, n)| camera::CameraInfo {
                index: i as u32,
                name: n.to_string(),
            })
            .collect()
    }

    #[test]
    fn camera_flag_accepts_number_or_name_fragment() {
        assert_eq!(choose_camera(&cams(), Some("2"), None).unwrap(), (1, false));
        assert_eq!(
            choose_camera(&cams(), Some("brio"), None).unwrap(),
            (1, false)
        );
        assert!(choose_camera(&cams(), Some("3"), None).is_err());
        assert!(choose_camera(&cams(), Some("0"), None).is_err());
        assert!(choose_camera(&cams(), Some("webcam"), None).is_err());
    }

    #[test]
    fn saved_camera_skips_the_prompt() {
        assert_eq!(
            choose_camera(&cams(), None, Some("Logitech BRIO")).unwrap(),
            (1, false)
        );
    }
}
