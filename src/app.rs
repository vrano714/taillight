use crate::indexer::LogIndexer;
use crate::parser::{parse_line, LogLevel};
use regex::Regex;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use serde::{Serialize, Deserialize};


#[derive(Clone)]
pub struct Tab {
    pub name: String,
    pub severity_filter: Option<LogLevel>,
    pub regex_filter: Option<String>,
    pub regex: Option<Regex>,
    pub filtered_indices: Vec<usize>,
    pub is_filtering: bool,
    pub last_processed_raw_idx: usize,
}

impl Tab {
    pub fn new(name: String, severity_filter: Option<LogLevel>, regex_filter: Option<String>) -> Self {
        let regex = regex_filter.as_ref().and_then(|s| Regex::new(s).ok());
        Self {
            name,
            severity_filter,
            regex_filter,
            regex,
            filtered_indices: Vec::new(),
            is_filtering: false,
            last_processed_raw_idx: 0,
        }
    }
}

pub struct PaneState {
    pub filter_index: usize,
    pub scroll_offset: usize,
    pub cursor_y: usize,
    pub highlight_query: Option<String>,
    pub highlight_regex: Option<Regex>,
    pub autoscroll: bool,
}

impl PaneState {
    pub fn new(filter_index: usize) -> Self {
        Self {
            filter_index,
            scroll_offset: 0,
            cursor_y: 0,
            highlight_query: None,
            highlight_regex: None,
            autoscroll: true,
        }
    }

    pub fn scroll_to_bottom(&mut self, total_lines: usize, viewport_height: usize) {
        if total_lines <= viewport_height {
            self.scroll_offset = 0;
            self.cursor_y = total_lines.saturating_sub(1);
        } else {
            self.scroll_offset = total_lines - viewport_height;
            self.cursor_y = viewport_height.saturating_sub(1);
        }
        self.autoscroll = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LayoutMode {
    Single,
    SplitVertical,
    SplitHorizontal,
    Split2x2,
}

impl LayoutMode {
    pub fn next(&self) -> Self {
        match self {
            LayoutMode::Single => LayoutMode::SplitVertical,
            LayoutMode::SplitVertical => LayoutMode::SplitHorizontal,
            LayoutMode::SplitHorizontal => LayoutMode::Split2x2,
            LayoutMode::Split2x2 => LayoutMode::Single,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    FilterInput(usize), // editing filter for tab_index
    HighlightInput,    // editing highlight for active pane
    TabNameInput,
    TabRegexInput(String), // holds the tab name while entering regex
    RenameTabInput(usize), // editing name of tab at index
    ExportConfigInput,     // editing filename to export config to
}

pub struct AppState {
    pub indexer: LogIndexer,
    pub tabs: Vec<Tab>,
    pub panes: Vec<PaneState>,
    pub active_pane_idx: usize,
    pub layout: LayoutMode,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub running: bool,
    pub status_message: Option<String>,
    
    // Background filter channels
    pub filter_tx: UnboundedSender<(usize, usize, Vec<usize>)>,
    pub filter_rx: UnboundedReceiver<(usize, usize, Vec<usize>)>,
    pub show_timestamps: bool,
    pub exited_via_ctrl_c: bool,
    pub ctrl_c_behavior: CtrlCBehavior,
    pub word_wrap: bool,
}

impl AppState {
    pub fn new(
        indexer: LogIndexer,
        custom_config_path: Option<std::path::PathBuf>,
        cli_show_timestamps: bool,
        cli_ctrl_c_behavior: Option<CtrlCBehavior>,
        cli_word_wrap: bool,
    ) -> Self {
        let (filter_tx, filter_rx) = unbounded_channel::<(usize, usize, Vec<usize>)>();
        
        let mut tabs = Vec::new();
        let mut panes = Vec::new();
        let mut layout = LayoutMode::Single;
        let mut active_pane_idx = 0;
        let mut loaded_path_msg = None;
        let mut show_timestamps = true;
        let mut ctrl_c_behavior = CtrlCBehavior::KillAll;
        let mut word_wrap = false;

        if let Some(custom_path) = custom_config_path {
            if custom_path.exists() {
                if let Ok(file) = std::fs::File::open(&custom_path) {
                    if let Ok(config) = serde_json::from_reader::<_, AppConfig>(file) {
                        for tc in config.filters {
                            tabs.push(Tab::new(tc.name, tc.severity_filter, tc.regex_filter));
                        }
                        for pc in config.panes {
                            let mut pane = PaneState::new(pc.filter_index);
                            if let Some(query) = pc.highlight_query {
                                pane.highlight_query = Some(query.clone());
                                pane.highlight_regex = Regex::new(&query).ok();
                            }
                            panes.push(pane);
                        }
                        layout = config.layout;
                        active_pane_idx = config.active_pane_idx;
                        show_timestamps = config.show_timestamps;
                        ctrl_c_behavior = config.ctrl_c_behavior;
                        word_wrap = config.word_wrap;
                        loaded_path_msg = Some(format!("Loaded config from {}", custom_path.to_string_lossy()));
                    }
                }
            }
        } else if let Some((config, path)) = load_config() {
            for tc in config.filters {
                tabs.push(Tab::new(tc.name, tc.severity_filter, tc.regex_filter));
            }
            for pc in config.panes {
                let mut pane = PaneState::new(pc.filter_index);
                if let Some(query) = pc.highlight_query {
                    pane.highlight_query = Some(query.clone());
                    pane.highlight_regex = Regex::new(&query).ok();
                }
                panes.push(pane);
            }
            layout = config.layout;
            active_pane_idx = config.active_pane_idx;
            show_timestamps = config.show_timestamps;
            ctrl_c_behavior = config.ctrl_c_behavior;
            word_wrap = config.word_wrap;
            loaded_path_msg = Some(format!("Loaded config from {}", path.to_string_lossy()));
        }

        if !cli_show_timestamps {
            show_timestamps = false;
        }

        if let Some(behavior) = cli_ctrl_c_behavior {
            ctrl_c_behavior = behavior;
        }

        if cli_word_wrap {
            word_wrap = true;
        }

        if tabs.is_empty() {
            tabs = vec![
                Tab::new("All".to_string(), None, None),
                Tab::new("Error".to_string(), Some(LogLevel::Error), None),
                Tab::new("Warn".to_string(), Some(LogLevel::Warn), None),
                Tab::new("Info".to_string(), Some(LogLevel::Info), None),
                Tab::new("Debug".to_string(), Some(LogLevel::Debug), None),
            ];
        }

        while panes.len() < 4 {
            panes.push(PaneState::new(0));
        }

        for pane in &mut panes {
            if pane.filter_index >= tabs.len() {
                pane.filter_index = 0;
            }
        }

        let status_msg = if let Some(msg) = loaded_path_msg {
            msg
        } else {
            "Press '?' or 'H' for help".to_string()
        };

        Self {
            indexer,
            tabs,
            panes,
            active_pane_idx,
            layout,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            running: true,
            status_message: Some(status_msg.to_string()),
            filter_tx,
            filter_rx,
            show_timestamps,
            exited_via_ctrl_c: false,
            ctrl_c_behavior,
            word_wrap,
        }
    }

    pub fn export_config_to_file(&mut self, filename: &str) {
        let mut export_path = std::path::PathBuf::from(filename);
        if export_path.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                export_path = cwd.join(export_path);
            }
        }

        let filters_config = self.tabs.iter().map(|t| FilterConfig {
            name: t.name.clone(),
            severity_filter: t.severity_filter,
            regex_filter: t.regex_filter.clone(),
        }).collect();

        let panes_config = self.panes.iter().map(|p| PaneConfig {
            filter_index: p.filter_index,
            highlight_query: p.highlight_query.clone(),
        }).collect();

        let config = AppConfig {
            layout: self.layout,
            active_pane_idx: self.active_pane_idx,
            filters: filters_config,
            panes: panes_config,
            show_timestamps: self.show_timestamps,
            ctrl_c_behavior: self.ctrl_c_behavior,
            word_wrap: self.word_wrap,
        };

        if let Some(parent) = export_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match std::fs::File::create(&export_path) {
            Ok(file) => {
                if serde_json::to_writer_pretty(file, &config).is_ok() {
                    self.status_message = Some(format!("Exported config to {}", export_path.to_string_lossy()));
                } else {
                    self.status_message = Some("Failed to serialize config".to_string());
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Failed to export config: {}", e));
            }
        }
    }

    /// Initialize the background tab filtering on startup
    pub async fn initialize_filters(&mut self) {
        for i in 0..self.tabs.len() {
            self.trigger_background_filter(i).await;
        }
    }

    /// Check if background filter tasks have returned results
    pub async fn handle_filter_updates(&mut self) {
        while let Ok((tab_idx, scan_limit, matched)) = self.filter_rx.try_recv() {
            if tab_idx >= self.tabs.len() {
                continue;
            }
            
            let tab = &mut self.tabs[tab_idx];
            tab.filtered_indices = matched;
            tab.is_filtering = false;
            tab.last_processed_raw_idx = scan_limit;
        }
    }

    /// Run filter check on raw indices between start and end
    /// Run filter check on raw indices between start and end
    async fn catch_up_tab(&mut self, tab_idx: usize, start: usize, end: usize) {
        let tab = &mut self.tabs[tab_idx];
        let severity = tab.severity_filter;
        let regex = tab.regex.clone();
        
        let (raw_offsets, file_len) = {
            let r = self.indexer.offsets.read().unwrap();
            let file_len = std::fs::metadata(&self.indexer.file_path).map(|m| m.len()).unwrap_or(0);
            (r[start..end].to_vec(), file_len)
        };

        let mut matches = Vec::new();
        let mut processed_count = 0;
        
        if let Ok(file) = std::fs::File::open(&self.indexer.file_path) {
            use std::io::{BufRead, BufReader, Seek, SeekFrom};
            let mut reader = BufReader::new(file);
            let mut line = String::new();
            for (offset_idx, &offset) in raw_offsets.iter().enumerate() {
                // If the offset is at or beyond the current file length, it points to a trailing EOF empty line.
                // Stop processing immediately so this offset can be retried in a later catch-up when it contains data!
                if offset >= file_len {
                    break;
                }
                
                if reader.seek(SeekFrom::Start(offset)).is_ok() {
                    line.clear();
                    if reader.read_line(&mut line).is_ok() {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        if test_filter(&line, &severity, &regex) {
                            matches.push(start + offset_idx);
                        }
                        processed_count += 1;
                    }
                }
            }
        }

        let tab = &mut self.tabs[tab_idx];
        tab.filtered_indices.extend(matches);
        tab.last_processed_raw_idx = start + processed_count;
    }

    /// Trigger filter computation for a tab in the background
    pub async fn trigger_background_filter(&mut self, tab_idx: usize) {
        let tab = &mut self.tabs[tab_idx];
        tab.is_filtering = true;
        tab.filtered_indices.clear();
        
        let scan_limit = self.indexer.line_count();
        let file_path = self.indexer.file_path.clone();
        let severity = tab.severity_filter;
        let regex = tab.regex.clone();
        let tx = self.filter_tx.clone();

        tokio::spawn(async move {
            let mut matched = Vec::new();
            let mut processed = 0;
            if let Ok(file) = std::fs::File::open(&file_path) {
                use std::io::{BufRead, BufReader};
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                
                while processed < scan_limit {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            if line.ends_with('\n') {
                                line.pop();
                                if line.ends_with('\r') {
                                    line.pop();
                                }
                            }
                            if test_filter(&line, &severity, &regex) {
                                matched.push(processed);
                            }
                            processed += 1;
                        }
                        Err(_) => break,
                    }
                    
                    if processed % 10000 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
            }
            let _ = tx.send((tab_idx, processed, matched));
        });
    }

    /// Check for newly appended logs and add them incrementally to all finished tabs
    pub async fn check_for_new_logs(&mut self) {
        let current_raw = self.indexer.line_count();
        for i in 0..self.tabs.len() {
            let (is_filtering, last_processed) = {
                let tab = &self.tabs[i];
                (tab.is_filtering, tab.last_processed_raw_idx)
            };
            
            if !is_filtering && current_raw > last_processed {
                self.catch_up_tab(i, last_processed, current_raw).await;
            }
        }
    }

    /// Update a tab's filter regex and trigger recalculation
    pub async fn update_tab_filter(&mut self, tab_idx: usize, regex_str: Option<String>) {
        {
            let tab = &mut self.tabs[tab_idx];
            tab.regex_filter = regex_str.clone();
            tab.regex = regex_str.as_ref().and_then(|s| Regex::new(s).ok());
            tab.filtered_indices.clear();
        }
        
        self.trigger_background_filter(tab_idx).await;
        
        let tab_name = self.tabs[tab_idx].name.clone();
        if let Some(ref r) = regex_str {
            self.status_message = Some(format!("Filter '{}' pattern updated to '{}'", tab_name, r));
        } else {
            self.status_message = Some(format!("Filter '{}' pattern cleared", tab_name));
        }
    }

    /// Move scrolling up
    pub fn scroll_up(&mut self, amount: usize) {
        let pane = &mut self.panes[self.active_pane_idx];
        pane.autoscroll = false;
        if pane.cursor_y >= amount {
            pane.cursor_y -= amount;
        } else {
            let needed = amount - pane.cursor_y;
            pane.cursor_y = 0;
            pane.scroll_offset = pane.scroll_offset.saturating_sub(needed);
        }
    }

    /// Move scrolling down
    pub fn scroll_down(&mut self, amount: usize, viewport_height: usize) {
        let pane = &mut self.panes[self.active_pane_idx];
        let total_lines = self.tabs[pane.filter_index].filtered_indices.len();
        
        if total_lines == 0 {
            pane.scroll_offset = 0;
            pane.cursor_y = 0;
            pane.autoscroll = true;
            return;
        }

        // Clamp viewport height
        let effective_h = viewport_height.min(total_lines);
        
        let current_line_idx = pane.scroll_offset + pane.cursor_y;
        let target_line_idx = (current_line_idx + amount).min(total_lines.saturating_sub(1));
        
        if target_line_idx >= total_lines.saturating_sub(1) {
            pane.autoscroll = true;
        }
        
        if target_line_idx < pane.scroll_offset + effective_h {
            pane.cursor_y = target_line_idx - pane.scroll_offset;
        } else {
            pane.scroll_offset = target_line_idx - effective_h + 1;
            pane.cursor_y = effective_h.saturating_sub(1);
        }
    }

    /// Jump to the top of the log
    pub fn jump_to_top(&mut self) {
        let pane = &mut self.panes[self.active_pane_idx];
        pane.scroll_offset = 0;
        pane.cursor_y = 0;
        pane.autoscroll = false;
    }

    /// Jump to the bottom of the log
    pub fn jump_to_bottom(&mut self, viewport_height: usize) {
        let pane = &mut self.panes[self.active_pane_idx];
        let total_lines = self.tabs[pane.filter_index].filtered_indices.len();
        pane.scroll_to_bottom(total_lines, viewport_height);
    }

    /// Next match in highlight search
    pub async fn jump_next_match(&mut self) {
        let pane = &mut self.panes[self.active_pane_idx];
        let tab = &self.tabs[pane.filter_index];
        let highlight_regex = match &pane.highlight_regex {
            Some(re) => re,
            None => return,
        };
        
        let total_lines = tab.filtered_indices.len();
        if total_lines == 0 { return; }
        
        let start_idx = pane.scroll_offset + pane.cursor_y + 1;
        
        for idx in start_idx..total_lines {
            let raw_idx = tab.filtered_indices[idx];
            let offset = {
                let r = self.indexer.offsets.read().unwrap();
                if raw_idx < r.len() { r[raw_idx] } else { continue }
            };
            if let Ok(line) = self.indexer.read_line(offset) {
                if highlight_regex.is_match(&line) {
                    pane.scroll_to_view_idx(idx);
                    self.status_message = Some(format!("Match found at line {}", idx + 1));
                    return;
                }
            }
        }
        
        // Wrap around
        for idx in 0..start_idx.min(total_lines) {
            let raw_idx = tab.filtered_indices[idx];
            let offset = {
                let r = self.indexer.offsets.read().unwrap();
                if raw_idx < r.len() { r[raw_idx] } else { continue }
            };
            if let Ok(line) = self.indexer.read_line(offset) {
                if highlight_regex.is_match(&line) {
                    pane.scroll_to_view_idx(idx);
                    self.status_message = Some(format!("Match wrapped to line {}", idx + 1));
                    return;
                }
            }
        }
        self.status_message = Some("No more matches found".to_string());
    }

    /// Previous match in highlight search
    pub async fn jump_prev_match(&mut self) {
        let pane = &mut self.panes[self.active_pane_idx];
        let tab = &self.tabs[pane.filter_index];
        let highlight_regex = match &pane.highlight_regex {
            Some(re) => re,
            None => return,
        };
        
        let total_lines = tab.filtered_indices.len();
        if total_lines == 0 { return; }
        
        let start_idx = (pane.scroll_offset + pane.cursor_y).saturating_sub(1);
        
        for idx in (0..=start_idx.min(total_lines - 1)).rev() {
            let raw_idx = tab.filtered_indices[idx];
            let offset = {
                let r = self.indexer.offsets.read().unwrap();
                if raw_idx < r.len() { r[raw_idx] } else { continue }
            };
            if let Ok(line) = self.indexer.read_line(offset) {
                if highlight_regex.is_match(&line) {
                    pane.scroll_to_view_idx(idx);
                    self.status_message = Some(format!("Match found at line {}", idx + 1));
                    return;
                }
            }
        }
        
        // Wrap around
        for idx in (start_idx..total_lines).rev() {
            let raw_idx = tab.filtered_indices[idx];
            let offset = {
                let r = self.indexer.offsets.read().unwrap();
                if raw_idx < r.len() { r[raw_idx] } else { continue }
            };
            if let Ok(line) = self.indexer.read_line(offset) {
                if highlight_regex.is_match(&line) {
                    pane.scroll_to_view_idx(idx);
                    self.status_message = Some(format!("Match wrapped to line {}", idx + 1));
                    return;
                }
            }
        }
        self.status_message = Some("No more matches found".to_string());
    }

    /// Add a new custom tab with search regex
    pub async fn create_custom_tab(&mut self, name: String, regex_str: String) {
        let name_trimmed = name.trim().to_string();
        let name_label = if name_trimmed.is_empty() {
            format!("*{}*", regex_str)
        } else {
            name_trimmed
        };
        
        let tab = Tab::new(name_label, None, Some(regex_str));
        self.tabs.push(tab);
        let new_tab_idx = self.tabs.len() - 1;
        
        // Point the active pane to the new tab
        self.panes[self.active_pane_idx].filter_index = new_tab_idx;
        
        self.trigger_background_filter(new_tab_idx).await;
        self.status_message = Some(format!("Created filter '{}'", self.tabs[new_tab_idx].name));
    }

    /// Delete the currently selected tab in the active pane (if it is a custom tab)
    pub async fn delete_current_tab(&mut self) {
        let active_tab_idx = self.panes[self.active_pane_idx].filter_index;
        
        // Cannot delete default tabs (0 to 4)
        if active_tab_idx < 5 {
            self.status_message = Some("Cannot delete built-in severity filters".to_string());
            return;
        }
        
        let tab_name = self.tabs[active_tab_idx].name.clone();
        self.tabs.remove(active_tab_idx);
        
        // Adjust tab indices for all panes pointing to deleted tab or after
        for pane in &mut self.panes {
            if pane.filter_index == active_tab_idx {
                pane.filter_index = 0; // Default to 'All'
            } else if pane.filter_index > active_tab_idx {
                pane.filter_index -= 1;
            }
        }
        
        self.status_message = Some(format!("Deleted filter '{}'", tab_name));
    }

    /// Rotate active pane focus
    pub fn cycle_pane(&mut self) {
        let max_panes = match self.layout {
            LayoutMode::Single => 1,
            LayoutMode::SplitVertical => 2,
            LayoutMode::SplitHorizontal => 2,
            LayoutMode::Split2x2 => 4,
        };
        
        self.active_pane_idx = (self.active_pane_idx + 1) % max_panes;
    }

    /// Cycle filters for the active pane
    pub async fn cycle_filter(&mut self, forward: bool) {
        let pane = &mut self.panes[self.active_pane_idx];
        let total_tabs = self.tabs.len();
        if total_tabs == 0 { return; }
        
        if forward {
            pane.filter_index = (pane.filter_index + 1) % total_tabs;
        } else {
            pane.filter_index = (pane.filter_index + total_tabs - 1) % total_tabs;
        }
        pane.scroll_offset = 0;
        pane.cursor_y = 0;
        pane.autoscroll = true;
    }
}

impl PaneState {
    pub fn scroll_to_view_idx(&mut self, idx: usize) {
        self.scroll_offset = idx.saturating_sub(10);
        self.cursor_y = idx.saturating_sub(self.scroll_offset);
        self.autoscroll = false;
    }
}

pub fn test_filter(
    line: &str,
    severity: &Option<LogLevel>,
    regex: &Option<Regex>,
) -> bool {
    let parsed = parse_line(line.to_string());
    

    if let Some(sev_limit) = severity {
        if *sev_limit == LogLevel::Error {
            if parsed.level != LogLevel::Error && parsed.level != LogLevel::Fatal {
                return false;
            }
        } else if parsed.level != *sev_limit {
            return false;
        }
    }
    
    if let Some(re) = regex {
        if !re.is_match(&parsed.raw) {
            return false;
        }
    }
    
    true
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FilterConfig {
    pub name: String,
    pub severity_filter: Option<LogLevel>,
    pub regex_filter: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PaneConfig {
    pub filter_index: usize,
    pub highlight_query: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CtrlCBehavior {
    KillAll,
    KillWriter,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub layout: LayoutMode,
    pub active_pane_idx: usize,
    pub filters: Vec<FilterConfig>,
    pub panes: Vec<PaneConfig>,
    #[serde(default = "default_show_timestamps")]
    pub show_timestamps: bool,
    #[serde(default = "default_ctrl_c_behavior")]
    pub ctrl_c_behavior: CtrlCBehavior,
    #[serde(default = "default_word_wrap")]
    pub word_wrap: bool,
}

fn default_show_timestamps() -> bool {
    true
}

fn default_ctrl_c_behavior() -> CtrlCBehavior {
    CtrlCBehavior::KillAll
}

fn default_word_wrap() -> bool {
    false
}

fn get_config_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let cwd_path = std::env::current_dir()
        .map(|d| d.join("taillight_config.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("taillight_config.json"));
    paths.push(cwd_path);
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = std::path::PathBuf::from(home);
        paths.push(home_path.join(".taillight_config.json"));
        paths.push(home_path.join(".config/taillight/config.json"));
    }
    paths
}

pub fn load_config() -> Option<(AppConfig, std::path::PathBuf)> {
    for path in get_config_paths() {
        if path.exists() {
            if let Ok(file) = std::fs::File::open(&path) {
                if let Ok(config) = serde_json::from_reader::<_, AppConfig>(file) {
                    return Some((config, path));
                }
            }
        }
    }
    None
}
