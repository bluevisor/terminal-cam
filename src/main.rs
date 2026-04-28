mod ascii;
mod camera;
mod color;
mod config;
mod menu;
mod render;
mod screenshot;
mod style;

use anyhow::{anyhow, Result};
use crossterm::{
    cursor,
    event::{poll, read, Event, KeyCode, KeyEventKind, KeyModifiers},
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

fn main() -> Result<()> {
    let cameras = camera::list_cameras()?;
    if cameras.is_empty() {
        return Err(anyhow!("no cameras found"));
    }

    let mut current_camera = pick_camera(&cameras)?;
    let mut capture = camera::spawn_capture(current_camera)?;
    let mut cfg = render::RenderConfig {
        detected: color::detect(),
        ..Default::default()
    };
    let mut app_config = config::AppConfig::load()?;
    let mut menu_state = menu::MenuState::new(
        cameras.clone(),
        current_camera,
        app_config.screenshot_dir.clone(),
    );
    let mut in_menu = false;

    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        Clear(ClearType::All),
        cursor::Hide
    )?;

    let result = run(
        &mut capture,
        &mut cfg,
        &mut menu_state,
        &mut current_camera,
        &mut in_menu,
        &mut app_config,
    );

    execute!(
        stdout(),
        cursor::Show,
        Clear(ClearType::All),
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;

    result
}

fn pick_camera(cameras: &[camera::CameraInfo]) -> Result<u32> {
    if cameras.len() == 1 {
        return Ok(cameras[0].index);
    }

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

fn run(
    capture: &mut camera::CaptureHandle,
    cfg: &mut render::RenderConfig,
    menu_state: &mut menu::MenuState,
    current_camera: &mut u32,
    in_menu: &mut bool,
    app_config: &mut config::AppConfig,
) -> Result<()> {
    let frame_budget = Duration::from_micros(1_000_000 / TARGET_FPS as u64);
    let mut scratch = String::with_capacity(1 << 18);
    let mut render_state = render::RenderState::new();
    let start = Instant::now();
    let mut status: Option<StatusMessage> = None;

    loop {
        let loop_start = Instant::now();
        let time = start.elapsed().as_secs_f32();
        let (cols, rows) = size().unwrap_or((80, 24));

        while poll(Duration::from_millis(0))? {
            match read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if k.code == KeyCode::Char(' ') && !menu_state.is_editing_text() {
                        let frame_opt = capture.frame.lock().clone();
                        set_status(
                            &mut status,
                            match frame_opt {
                                Some(frame) => match screenshot::save(
                                    &frame,
                                    cols,
                                    rows,
                                    cfg,
                                    time,
                                    &app_config.screenshot_dir,
                                ) {
                                    Ok(path) => format!("screenshot saved: {}", path.display()),
                                    Err(err) => format!("screenshot failed: {err:#}"),
                                },
                                None => "screenshot failed: no camera frame yet".to_string(),
                            },
                        );
                        continue;
                    }

                    if *in_menu {
                        match menu_state.on_key(k) {
                            menu::Action::None => {}
                            menu::Action::Close => {
                                *in_menu = false;
                                execute!(stdout(), Clear(ClearType::All))?;
                            }
                            menu::Action::Quit => return Ok(()),
                            menu::Action::SwitchCamera(idx) => {
                                if idx != *current_camera {
                                    capture.stop();
                                    *capture = camera::spawn_capture(idx)?;
                                    *current_camera = idx;
                                }
                            }
                            menu::Action::CycleStyle(dir) => {
                                cfg.style = cfg.style.cycle(dir);
                            }
                            menu::Action::CycleMode(dir) => {
                                cfg.mode = cfg.mode.cycle(dir);
                                execute!(stdout(), Clear(ClearType::All))?;
                            }
                            menu::Action::CycleDepth(dir) => {
                                cfg.depth = cfg.depth.cycle(dir);
                                execute!(stdout(), Clear(ClearType::All))?;
                            }
                            menu::Action::ToggleMirror => cfg.mirror = !cfg.mirror,
                            menu::Action::AdjustBrightness(d) => {
                                cfg.brightness = (cfg.brightness + d).clamp(-1.0, 1.0);
                            }
                            menu::Action::AdjustContrast(d) => {
                                cfg.contrast = (cfg.contrast + d).clamp(0.1, 3.0);
                            }
                            menu::Action::SetScreenshotDir(input) => {
                                let mut next = app_config.clone();
                                let result = next
                                    .set_screenshot_dir_from_input(&input)
                                    .and_then(|_| next.save());
                                match result {
                                    Ok(()) => {
                                        *app_config = next;
                                        menu_state
                                            .set_screenshot_dir(app_config.screenshot_dir.clone());
                                        set_status(
                                            &mut status,
                                            format!(
                                                "screenshot path saved: {}",
                                                app_config.screenshot_dir.display()
                                            ),
                                        );
                                    }
                                    Err(err) => {
                                        set_status(
                                            &mut status,
                                            format!("screenshot path failed: {err:#}"),
                                        );
                                    }
                                }
                            }
                        }
                    } else {
                        match (k.code, k.modifiers) {
                            (KeyCode::Esc, _) => {
                                *in_menu = true;
                                menu_state.current_camera = *current_camera;
                            }
                            (KeyCode::Char('q'), _)
                            | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),
                            _ => {}
                        }
                    }
                }
                Event::Resize(_, _) => {
                    execute!(stdout(), Clear(ClearType::All))?;
                }
                _ => {}
            }
        }

        if status
            .as_ref()
            .is_some_and(|message| Instant::now() >= message.until)
        {
            status = None;
        }

        let frame_opt = capture.frame.lock().clone();
        if let Some(frame) = frame_opt {
            render::render(&frame, cols, rows, cfg, time, &mut render_state, &mut scratch);
            if *in_menu {
                menu::draw(menu_state, cfg, cols, rows, &mut scratch);
            }
            if let Some(message) = &status {
                draw_status(message, cols, rows, &mut scratch);
            }
            render::flush(&scratch)?;
        } else {
            let mut o = stdout().lock();
            write!(o, "\x1b[H\x1b[0mwaiting for camera…\x1b[0K")?;
            o.flush()?;
        }

        let elapsed = loop_start.elapsed();
        if elapsed < frame_budget {
            std::thread::sleep(frame_budget - elapsed);
        }
    }
}

struct StatusMessage {
    text: String,
    until: Instant,
}

fn set_status(status: &mut Option<StatusMessage>, text: String) {
    *status = Some(StatusMessage {
        text,
        until: Instant::now() + Duration::from_secs(3),
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
