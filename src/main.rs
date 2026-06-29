use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use regex::Regex;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod app;
mod indexer;
mod parser;
mod ui;

use app::{AppState, InputMode, LayoutMode};
use indexer::LogIndexer;
use ui::draw;

#[derive(Parser, Debug)]
#[command(name = "taillight", version = "0.1.0", about = "Real-Time Log Viewer")]
struct Args {
    /// Path to the log file. If omitted, taillight reads from standard input.
    path: Option<String>,

    /// Initial layout mode: single, vertical, horizontal, split2x2
    #[arg(short, long)]
    layout: Option<String>,

    /// Path to config file to load/save.
    #[arg(short, long)]
    config: Option<String>,

    /// Hide timestamps in log rendering
    #[arg(long)]
    no_timestamps: bool,

    /// Ctrl-C behavior: kill-all (stops pipeline and exits), kill-writer (stops pipeline but keeps taillight open)
    #[arg(long, default_value = "kill-all")]
    ctrl_c_behavior: String,

    /// Enable word wrapping for long log lines
    #[arg(short, long)]
    word_wrap: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Determine input source (file vs stdin)
    let indexer = if let Some(ref path_str) = args.path {
        let path = PathBuf::from(path_str);
        if !path.exists() {
            eprintln!("Error: File '{}' does not exist.", path_str);
            std::process::exit(1);
        }
        LogIndexer::new_file(path)
    } else {
        // Read from stdin if piped
        if !io::stdin().is_terminal() {
            LogIndexer::new_stdin()?
        } else {
            eprintln!("Error: No log file specified and stdin is not piped.");
            eprintln!("Usage: taillight <file> or cat <file> | taillight");
            std::process::exit(1);
        }
    };

    // Start background file/stdin reading and indexing
    indexer.start_indexing();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create AppState
    let custom_config_path = args.config.map(PathBuf::from);
    let cli_ctrl_c_behavior = match args.ctrl_c_behavior.to_lowercase().as_str() {
        "kill-writer" | "writer" => Some(app::CtrlCBehavior::KillWriter),
        "kill-all" | "all" => Some(app::CtrlCBehavior::KillAll),
        _ => None,
    };
    let mut app = AppState::new(
        indexer,
        custom_config_path,
        !args.no_timestamps,
        cli_ctrl_c_behavior,
        args.word_wrap,
    );
    
    // Set initial layout if explicitly passed as argument
    if let Some(ref layout_str) = args.layout {
        app.layout = match layout_str.to_lowercase().as_str() {
            "single" | "s" => LayoutMode::Single,
            "vertical" | "v" => LayoutMode::SplitVertical,
            "horizontal" | "h" => LayoutMode::SplitHorizontal,
            "split2x2" | "2x2" | "grid" => LayoutMode::Split2x2,
            _ => app.layout,
        };
    }

    // Run initial filter calculations
    app.initialize_filters().await;

    // Run TUI loop
    let mut last_draw = Instant::now();
    
    while app.running {
        // Poll for filter updates from background threads
        app.handle_filter_updates().await;

        // Check if logs have grown in the indexer
        app.check_for_new_logs().await;

        // Poll terminal keyboard/resize events
        if event::poll(Duration::from_millis(5))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        let height = terminal.size()?.height as usize;
                        let viewport_height = height.saturating_sub(3); // header, status, command take 3 lines
                        handle_key_event(&mut app, key, viewport_height).await;
                    }
                }
                Event::Resize(_, _) => {
                    // Force redraw on resize
                    terminal.draw(|f| draw(f, &mut app))?;
                }
                _ => {}
            }
        }

        // Draw screen at ~50fps
        if last_draw.elapsed() >= Duration::from_millis(20) {
            terminal.draw(|f| draw(f, &mut app))?;
            last_draw = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    #[cfg(unix)]
    if app.exited_via_ctrl_c || app.indexer.is_stdin() {
        unsafe {
            let pgid = libc::getpgrp();
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::kill(-pgid, libc::SIGINT);
        }
    }

    Ok(())
}

async fn handle_key_event(app: &mut AppState, key: KeyEvent, viewport_height: usize) {
    if key.code == event::KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
        match app.ctrl_c_behavior {
            app::CtrlCBehavior::KillAll => {
                app.exited_via_ctrl_c = true;
                app.running = false;
            }
            app::CtrlCBehavior::KillWriter => {
                #[cfg(unix)]
                unsafe {
                    let pgid = libc::getpgrp();
                    libc::signal(libc::SIGINT, libc::SIG_IGN);
                    libc::kill(-pgid, libc::SIGINT);
                    libc::signal(libc::SIGINT, libc::SIG_DFL);
                }
                app.status_message = Some("Stopped log stream (sent SIGINT to pipeline)".to_string());
            }
        }
        return;
    }

    match app.input_mode {
        InputMode::Normal => {
            match key.code {
                KeyCode::Char('q') => {
                    app.running = false;
                }
                KeyCode::Char('?') | KeyCode::Char('H') => {
                    app.status_message = Some(
                        "[/] Filter | [s] Highlight | [f] New Filter | [t] Toggle Time | [C] Ctrl-C Mode | [w] Toggle Wrap | [r] Rename Filter | [e] Export Config | [x] Close Filter | [Tab] Focus Pane | [v] Layout | [Esc] Normal | [q] Quit".to_string()
                    );
                }
                KeyCode::Char('/') => {
                    let active_tab = app.panes[app.active_pane_idx].filter_index;
                    app.input_mode = InputMode::FilterInput(active_tab);
                    app.input_buffer = app.tabs[active_tab].regex_filter.clone().unwrap_or_default();
                }
                KeyCode::Char('s') => {
                    app.input_mode = InputMode::HighlightInput;
                    app.input_buffer = app.panes[app.active_pane_idx].highlight_query.clone().unwrap_or_default();
                }
                KeyCode::Char('f') => {
                    app.input_mode = InputMode::TabNameInput;
                    app.input_buffer = String::new();
                }
                KeyCode::Char('t') => {
                    app.show_timestamps = !app.show_timestamps;
                    app.status_message = Some(format!(
                        "Timestamps {}",
                        if app.show_timestamps { "enabled" } else { "disabled" }
                    ));
                }
                KeyCode::Char('C') => {
                    app.ctrl_c_behavior = match app.ctrl_c_behavior {
                        app::CtrlCBehavior::KillAll => app::CtrlCBehavior::KillWriter,
                        app::CtrlCBehavior::KillWriter => app::CtrlCBehavior::KillAll,
                    };
                    app.status_message = Some(format!(
                        "Ctrl-C behavior set to: {}",
                        match app.ctrl_c_behavior {
                            app::CtrlCBehavior::KillAll => "Kill All & Exit",
                            app::CtrlCBehavior::KillWriter => "Kill Writer Only",
                        }
                    ));
                }
                KeyCode::Char('w') => {
                    app.word_wrap = !app.word_wrap;
                    app.status_message = Some(format!(
                        "Word wrap {}",
                        if app.word_wrap { "enabled" } else { "disabled" }
                    ));
                }
                KeyCode::Char('r') => {
                    let active_tab = app.panes[app.active_pane_idx].filter_index;
                    app.input_mode = InputMode::RenameTabInput(active_tab);
                    app.input_buffer = app.tabs[active_tab].name.clone();
                }
                KeyCode::Char('e') => {
                    app.input_mode = InputMode::ExportConfigInput;
                    app.input_buffer = "taillight_config.json".to_string();
                }
                KeyCode::Char('c') | KeyCode::Char('x') => {
                    app.delete_current_tab().await;
                }
                KeyCode::Tab => {
                    app.cycle_pane();
                }
                KeyCode::BackTab => {
                    let max_panes = match app.layout {
                        LayoutMode::Single => 1,
                        LayoutMode::SplitVertical => 2,
                        LayoutMode::SplitHorizontal => 2,
                        LayoutMode::Split2x2 => 4,
                    };
                    app.active_pane_idx = (app.active_pane_idx + max_panes - 1) % max_panes;
                }
                KeyCode::Char('v') => {
                    app.layout = app.layout.next();
                    let max_panes = match app.layout {
                        LayoutMode::Single => 1,
                        LayoutMode::SplitVertical => 2,
                        LayoutMode::SplitHorizontal => 2,
                        LayoutMode::Split2x2 => 4,
                    };
                    if app.active_pane_idx >= max_panes {
                        app.active_pane_idx = 0;
                    }
                    app.status_message = Some(format!("Layout: {:?}", app.layout));
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    app.cycle_filter(true).await;
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    app.cycle_filter(false).await;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.scroll_up(1);
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    app.scroll_down(1, viewport_height);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.scroll_up(viewport_height / 2);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.scroll_down(viewport_height / 2, viewport_height);
                }
                KeyCode::Char('g') => {
                    app.jump_to_top();
                }
                KeyCode::Char('G') => {
                    app.jump_to_bottom(viewport_height);
                }
                KeyCode::Char('n') => {
                    app.jump_next_match().await;
                }
                KeyCode::Char('N') => {
                    app.jump_prev_match().await;
                }
                KeyCode::Char(c) if c.is_digit(10) => {
                    let tab_idx = c.to_digit(10).unwrap() as usize;
                    if tab_idx < app.tabs.len() {
                        let pane = &mut app.panes[app.active_pane_idx];
                        pane.filter_index = tab_idx;
                        pane.scroll_offset = 0;
                        pane.cursor_y = 0;
                        pane.autoscroll = true;
                    }
                }
                _ => {}
            }
        }
        InputMode::FilterInput(tab_idx) => {
            match key.code {
                KeyCode::Enter => {
                    let query = app.input_buffer.clone();
                    let regex_str = if query.trim().is_empty() { None } else { Some(query) };
                    app.update_tab_filter(tab_idx, regex_str).await;
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                _ => {}
            }
        }
        InputMode::HighlightInput => {
            match key.code {
                KeyCode::Enter => {
                    let query = app.input_buffer.clone();
                    let pane = &mut app.panes[app.active_pane_idx];
                    if query.trim().is_empty() {
                        pane.highlight_query = None;
                        pane.highlight_regex = None;
                    } else if let Ok(re) = Regex::new(&query) {
                        pane.highlight_query = Some(query);
                        pane.highlight_regex = Some(re);
                    } else {
                        app.status_message = Some("Invalid regex".to_string());
                    }
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                _ => {}
            }
        }
        InputMode::TabNameInput => {
            match key.code {
                KeyCode::Enter => {
                    let name = app.input_buffer.clone();
                    app.input_mode = InputMode::TabRegexInput(name);
                    app.input_buffer = String::new();
                }
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                _ => {}
            }
        }
        InputMode::TabRegexInput(ref name) => {
            match key.code {
                KeyCode::Enter => {
                    let regex_str = app.input_buffer.clone();
                    if Regex::new(&regex_str).is_ok() {
                        let name_cloned = name.clone();
                        app.create_custom_tab(name_cloned, regex_str).await;
                        app.input_mode = InputMode::Normal;
                    } else {
                        app.status_message = Some("Invalid regex. Try again.".to_string());
                    }
                }
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                _ => {}
            }
        }
        InputMode::RenameTabInput(tab_idx) => {
            match key.code {
                KeyCode::Enter => {
                    let new_name = app.input_buffer.trim().to_string();
                    if !new_name.is_empty() {
                        app.tabs[tab_idx].name = new_name;
                        app.input_mode = InputMode::Normal;
                    } else {
                        app.status_message = Some("Name cannot be empty".to_string());
                    }
                }
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                _ => {}
            }
        }
        InputMode::ExportConfigInput => {
            match key.code {
                KeyCode::Enter => {
                    let filename = app.input_buffer.trim().to_string();
                    if !filename.is_empty() {
                        app.export_config_to_file(&filename);
                        app.input_mode = InputMode::Normal;
                    } else {
                        app.status_message = Some("Filename cannot be empty".to_string());
                    }
                }
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                _ => {}
            }
        }
    }
}
