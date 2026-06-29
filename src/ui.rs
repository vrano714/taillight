use crate::app::{AppState, LayoutMode};
use crate::parser::{parse_line, LogLevel};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Paragraph},
    Frame,
};
use regex::Regex;

pub fn draw(f: &mut Frame, app: &mut AppState) {
    // Overall vertical layout: 
    // 1. Top Bar (Tabs, only shown in Single mode, or always for reference)
    // 2. Middle Area (Pulp of logs / splits)
    // 3. Status Bar (Log metadata)
    // 4. Input Area (Active command line)
    
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Top header
            Constraint::Min(3),    // Middle log area
            Constraint::Length(1), // Status bar
            Constraint::Length(1), // Command/Input bar
        ])
        .split(f.size());

    // --- 1. RENDER TOP HEADER ---
    render_header(f, app, main_layout[0]);

    // --- 2. RENDER MIDDLE LOG VIEWER (SINGLE OR SPLIT PANES) ---
    render_log_panes(f, app, main_layout[1]);

    // --- 3. RENDER STATUS BAR ---
    render_status_bar(f, app, main_layout[2]);

    // --- 4. RENDER INPUT AREA ---
    render_input_bar(f, app, main_layout[3]);
}

fn render_header(f: &mut Frame, app: &AppState, area: Rect) {
    let mut spans = Vec::new();
    spans.push(Span::styled(" TAILLIGHT ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)));
    spans.push(Span::raw(" | "));
    
    for (i, tab) in app.tabs.iter().enumerate() {
        let is_active_tab = match app.layout {
            LayoutMode::Single => app.panes[0].filter_index == i,
            LayoutMode::SplitVertical | LayoutMode::SplitHorizontal => {
                app.panes[0].filter_index == i || app.panes[1].filter_index == i
            }
            LayoutMode::Split2x2 => {
                app.panes.iter().any(|p| p.filter_index == i)
            }
        };
        
        let mut tab_style = Style::default();
        if is_active_tab {
            tab_style = tab_style.fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        } else {
            tab_style = tab_style.fg(Color::Gray);
        }
        
        let tab_label = if tab.is_filtering {
            format!("[{i}:{}...]", tab.name)
        } else {
            format!("[{i}:{}]", tab.name)
        };
        
        spans.push(Span::styled(tab_label, tab_style));
        spans.push(Span::raw(" "));
    }
    
    let header = Paragraph::new(Line::from(spans));
    f.render_widget(header, area);
}

fn render_log_panes(f: &mut Frame, app: &mut AppState, area: Rect) {
    match app.layout {
        LayoutMode::Single => {
            render_pane(f, app, 0, area);
        }
        LayoutMode::SplitVertical => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            render_pane(f, app, 0, chunks[0]);
            render_pane(f, app, 1, chunks[1]);
        }
        LayoutMode::SplitHorizontal => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            render_pane(f, app, 0, chunks[0]);
            render_pane(f, app, 1, chunks[1]);
        }
        LayoutMode::Split2x2 => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            let top_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[0]);
            let bot_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[1]);
                
            render_pane(f, app, 0, top_cols[0]);
            render_pane(f, app, 1, top_cols[1]);
            render_pane(f, app, 2, bot_cols[0]);
            render_pane(f, app, 3, bot_cols[1]);
        }
    }
}

fn render_pane(f: &mut Frame, app: &mut AppState, pane_idx: usize, area: Rect) {
    let is_focused = app.active_pane_idx == pane_idx;
    
    let pane_state_idx = if app.layout == LayoutMode::Single {
        app.active_pane_idx
    } else {
        pane_idx
    };
    
    let (scroll_offset, autoscroll, _cursor_y, tab_idx) = {
        let pane = &app.panes[pane_state_idx];
        let total_lines = app.tabs[pane.filter_index].filtered_indices.len();
        let block_inner_area = Block::default().borders(Borders::ALL).inner(area);
        let viewport_height = block_inner_area.height as usize;
        
        let scroll_offset = if pane.autoscroll {
            total_lines.saturating_sub(viewport_height)
        } else {
            pane.scroll_offset.min(total_lines.saturating_sub(1))
        };
        (scroll_offset, pane.autoscroll, pane.cursor_y, pane.filter_index)
    };
    
    if app.layout == LayoutMode::Single {
        app.panes[app.active_pane_idx].scroll_offset = scroll_offset;
    } else {
        app.panes[pane_idx].scroll_offset = scroll_offset;
    }
    
    let pane = &app.panes[pane_state_idx];
    let tab = &app.tabs[tab_idx];
    
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    
    let title = format!(
        " Pane {} [Filter: {}] {} ",
        pane_state_idx + 1,
        tab.name,
        if autoscroll { "[TAIL]" } else { "" }
    );
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title);
        
    let inner_area = block.inner(area);
    let viewport_height = inner_area.height as usize;
    
    let total_lines = tab.filtered_indices.len();
    
    let lines_to_read = viewport_height.min(total_lines - scroll_offset);
    let visible_indices = &tab.filtered_indices[scroll_offset..(scroll_offset + lines_to_read)];
    
    let mut log_lines = Vec::new();
    if !visible_indices.is_empty() {
        let offsets = {
            let r = app.indexer.offsets.read().unwrap();
            visible_indices.iter().filter_map(|&idx| {
                if idx < r.len() { Some(r[idx]) } else { None }
            }).collect::<Vec<u64>>()
        };
        
        if let Ok(lines) = app.indexer.read_lines(&offsets) {
            for (i, line_str) in lines.iter().enumerate() {
                let view_idx = scroll_offset + i;
                let is_cursor_line = is_focused && (view_idx == scroll_offset + pane.cursor_y);
                
                let formatted = format_log_line(line_str, &pane.highlight_regex, is_cursor_line, app.show_timestamps);
                log_lines.push(formatted);
            }
        }
    }
    
    while log_lines.len() < viewport_height {
        log_lines.push(Line::from(vec![Span::raw("")]));
    }
    
    let mut paragraph = Paragraph::new(log_lines).block(block);
    if app.word_wrap {
        paragraph = paragraph.wrap(ratatui::widgets::Wrap { trim: false });
    }
    f.render_widget(paragraph, area);
}

fn render_status_bar(f: &mut Frame, app: &AppState, area: Rect) {
    let pane = &app.panes[app.active_pane_idx];
    let tab = &app.tabs[pane.filter_index];
    
    let path_str = app.indexer.file_path.to_string_lossy();
    let total_raw = app.indexer.line_count();
    let total_filtered = tab.filtered_indices.len();
    
    let status_text = format!(
        " File: {} | Total: {} | Matches: {} | Active Pane: P{} [Filter: {}] | Layout: {:?}",
        path_str,
        total_raw,
        total_filtered,
        app.active_pane_idx + 1,
        tab.name,
        app.layout
    );
    
    let bar = Paragraph::new(Line::from(vec![
        Span::styled(status_text, Style::default().fg(Color::Black).bg(Color::White))
    ]));
    
    f.render_widget(bar, area);
}

fn render_input_bar(f: &mut Frame, app: &AppState, area: Rect) {
    use crate::app::InputMode;
    let content = match &app.input_mode {
        InputMode::Normal => {
            let msg = app.status_message.as_deref().unwrap_or("Press '?' or 'h' for help");
            Line::from(vec![
                Span::styled(" STATUS: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(msg)
            ])
        }
        InputMode::FilterInput(tab_idx) => {
            let tab = &app.tabs[*tab_idx];
            Line::from(vec![
                Span::styled(" FILTER [", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(&tab.name, Style::default().fg(Color::Yellow)),
                Span::styled("]: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(&app.input_buffer)
            ])
        }
        InputMode::HighlightInput => {
            Line::from(vec![
                Span::styled(" HIGHLIGHT (Active Pane): ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                Span::raw(&app.input_buffer)
            ])
        }
        InputMode::TabNameInput => {
            Line::from(vec![
                Span::styled(" NEW FILTER NAME: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(&app.input_buffer)
            ])
        }
        InputMode::TabRegexInput(name) => {
            Line::from(vec![
                Span::styled(format!(" NEW FILTER '{}' REGEX: ", name), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(&app.input_buffer)
            ])
        }
        InputMode::RenameTabInput(tab_idx) => {
            let tab = &app.tabs[*tab_idx];
            Line::from(vec![
                Span::styled(format!(" RENAME FILTER '{}' TO: ", tab.name), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(&app.input_buffer)
            ])
        }
        InputMode::ExportConfigInput => {
            Line::from(vec![
                Span::styled(" EXPORT CONFIG TO: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(&app.input_buffer)
            ])
        }
    };
    
    f.render_widget(Paragraph::new(content), area);
}

pub fn format_log_line(
    line_str: &str,
    highlight_re: &Option<Regex>,
    is_selected: bool,
    show_timestamps: bool,
) -> Line<'static> {
    let parsed = parse_line(line_str.to_string());
    
    let level_color = match parsed.level {
        LogLevel::Trace => Color::Magenta,
        LogLevel::Debug => Color::Blue,
        LogLevel::Info => Color::Green,
        LogLevel::Warn => Color::Yellow,
        LogLevel::Error | LogLevel::Fatal => Color::Red,
        LogLevel::Unknown => Color::White,
    };
    
    let level_style = Style::default().fg(level_color).add_modifier(Modifier::BOLD);
    let ts_style = Style::default().fg(Color::DarkGray);
    
    let mut base_text_style = Style::default();
    if is_selected {
        base_text_style = base_text_style.bg(Color::Rgb(50, 50, 50)).fg(Color::White).add_modifier(Modifier::BOLD);
    }
    
    let hl_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Rgb(255, 180, 0))
        .add_modifier(Modifier::BOLD);
        
    let mut spans = Vec::new();
    
    if show_timestamps {
        if let Some(ref ts) = parsed.timestamp {
            spans.push(Span::styled(format!("{} ", ts), ts_style));
        }
    }
    
    if parsed.level != LogLevel::Unknown {
        spans.push(Span::styled(format!("[{}] ", parsed.level.as_str()), level_style));
    }
    
    let msg_spans = highlight_spans(&parsed.message, highlight_re, base_text_style, hl_style);
    spans.extend(msg_spans);
    
    Line::from(spans)
}

fn highlight_spans(
    text: &str,
    re: &Option<Regex>,
    base_style: Style,
    hl_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(regex) = re {
        let mut last_match = 0;
        for mat in regex.find_iter(text) {
            let start = mat.start();
            let end = mat.end();
            if start > last_match {
                spans.push(Span::styled(text[last_match..start].to_string(), base_style));
            }
            spans.push(Span::styled(text[start..end].to_string(), hl_style));
            last_match = end;
        }
        if last_match < text.len() {
            spans.push(Span::styled(text[last_match..].to_string(), base_style));
        }
    } else {
        spans.push(Span::styled(text.to_string(), base_style));
    }
    spans
}
