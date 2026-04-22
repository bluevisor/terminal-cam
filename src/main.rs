mod ascii;
mod camera;
mod color;
mod menu;
mod render;
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
use std::io::{stdout, Write};
use std::time::{Duration, Instant};

const TARGET_FPS: u32 = 30;

fn main() -> Result<()> {
    let cameras = camera::list_cameras()?;
    if cameras.is_empty() {
        return Err(anyhow!("no cameras found"));
    }

    let mut current_camera = cameras[0].index;
    let mut capture = camera::spawn_capture(current_camera)?;
    let mut cfg = render::RenderConfig {
        detected: color::detect(),
        ..Default::default()
    };
    let mut menu_state = menu::MenuState::new(cameras.clone(), current_camera);
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

fn run(
    capture: &mut camera::CaptureHandle,
    cfg: &mut render::RenderConfig,
    menu_state: &mut menu::MenuState,
    current_camera: &mut u32,
    in_menu: &mut bool,
) -> Result<()> {
    let frame_budget = Duration::from_micros(1_000_000 / TARGET_FPS as u64);
    let mut scratch = String::with_capacity(1 << 18);
    let start = Instant::now();

    loop {
        let loop_start = Instant::now();
        let (cols, rows) = size().unwrap_or((80, 24));

        while poll(Duration::from_millis(0))? {
            match read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
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

        let time = start.elapsed().as_secs_f32();
        let frame_opt = capture.frame.lock().clone();
        if let Some(frame) = frame_opt {
            render::render(&frame, cols, rows, cfg, time, &mut scratch);
            if *in_menu {
                menu::draw(menu_state, cfg, cols, rows, &mut scratch);
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
