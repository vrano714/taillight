use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    Unknown,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
            LogLevel::Unknown => "LOG",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "TRACE" | "TRC" => LogLevel::Trace,
            "DEBUG" | "DBG" => LogLevel::Debug,
            "INFO" | "INF" => LogLevel::Info,
            "WARN" | "WRN" | "WARNING" => LogLevel::Warn,
            "ERROR" | "ERR" => LogLevel::Error,
            "FATAL" | "FTL" => LogLevel::Fatal,
            _ => LogLevel::Unknown,
        }
    }
}

pub struct ParsedLine {
    pub level: LogLevel,
    pub timestamp: Option<String>,
    pub message: String,
    pub raw: String,
}

pub fn parse_line(line: String) -> ParsedLine {
    let raw = line.clone();
    
    // Check if it's JSON
    if line.trim_start().starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(obj) = value.as_object() {
                // Try to find the level
                let level = obj.get("level")
                    .or_else(|| obj.get("severity"))
                    .or_else(|| obj.get("lvl"))
                    .and_then(|v| v.as_str())
                    .map(LogLevel::from_str)
                    .unwrap_or(LogLevel::Unknown);

                // Try to find timestamp
                let timestamp = obj.get("time")
                    .or_else(|| obj.get("timestamp"))
                    .or_else(|| obj.get("@timestamp"))
                    .or_else(|| obj.get("t"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // Try to find message
                let msg_key = if obj.contains_key("message") { Some("message") }
                    else if obj.contains_key("msg") { Some("msg") }
                    else if obj.contains_key("body") { Some("body") }
                    else if obj.contains_key("log") { Some("log") }
                    else { None };

                let message = if let Some(key) = msg_key {
                    let msg_val = obj.get(key).unwrap();
                    let msg_str = if msg_val.is_string() {
                        msg_val.as_str().unwrap().to_string()
                    } else {
                        msg_val.to_string()
                    };

                    // Check if there are other custom fields
                    let mut extra_obj = obj.clone();
                    extra_obj.remove("level");
                    extra_obj.remove("severity");
                    extra_obj.remove("lvl");
                    extra_obj.remove("time");
                    extra_obj.remove("timestamp");
                    extra_obj.remove("@timestamp");
                    extra_obj.remove("t");
                    extra_obj.remove(key);

                    if !extra_obj.is_empty() {
                        format!("{} {}", msg_str, serde_json::Value::Object(extra_obj))
                    } else {
                        msg_str
                    }
                } else {
                    // Fallback: use the raw JSON without the keys we extracted
                    let mut filtered_obj = obj.clone();
                    filtered_obj.remove("level");
                    filtered_obj.remove("severity");
                    filtered_obj.remove("lvl");
                    filtered_obj.remove("time");
                    filtered_obj.remove("timestamp");
                    filtered_obj.remove("@timestamp");
                    filtered_obj.remove("t");
                    serde_json::Value::Object(filtered_obj).to_string()
                };

                return ParsedLine {
                    level,
                    timestamp,
                    message,
                    raw,
                };
            }
        }
    }

    // Otherwise, parse as Plaintext
    parse_plaintext(raw)
}

fn parse_plaintext(raw: String) -> ParsedLine {
    let mut level = LogLevel::Unknown;
    let upper = raw.to_uppercase();
    
    // Check common patterns for severity levels
    for &lvl in &[LogLevel::Fatal, LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
        let variants = match lvl {
            LogLevel::Fatal => &["FATAL", "FTL"][..],
            LogLevel::Error => &["ERROR", "ERR"][..],
            LogLevel::Warn => &["WARN", "WARNING", "WRN"][..],
            LogLevel::Info => &["INFO", "INF"][..],
            LogLevel::Debug => &["DEBUG", "DBG"][..],
            LogLevel::Trace => &["TRACE", "TRC"][..],
            LogLevel::Unknown => &[][..],
        };
        
        let mut matched = false;
        for &name in variants {
            if upper.contains(&format!("[{}]", name)) 
                || upper.contains(&format!(" {} ", name))
                || upper.contains(&format!("{}:", name))
                || upper.contains(&format!("<{}>", name))
            {
                level = lvl;
                matched = true;
                break;
            }
        }
        if matched {
            break;
        }
    }
    
    if level == LogLevel::Unknown {
        for &lvl in &[LogLevel::Fatal, LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
            let variants = match lvl {
                LogLevel::Fatal => &["FATAL", "FTL"][..],
                LogLevel::Error => &["ERROR", "ERR"][..],
                LogLevel::Warn => &["WARN", "WARNING", "WRN"][..],
                LogLevel::Info => &["INFO", "INF"][..],
                LogLevel::Debug => &["DEBUG", "DBG"][..],
                LogLevel::Trace => &["TRACE", "TRC"][..],
                LogLevel::Unknown => &[][..],
            };
            
            let mut matched = false;
            for &name in variants {
                if upper.starts_with(name) {
                    level = lvl;
                    matched = true;
                    break;
                }
            }
            if matched {
                break;
            }
        }
    }

    // Simple regex pattern matching common timestamp formats at the start of a log line
    thread_local! {
        static TIMESTAMP_REGEX: regex::Regex = regex::Regex::new(
            r"(?i)^[\[\(]?(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)[\]\)]?"
        ).unwrap();
    }

    let timestamp = TIMESTAMP_REGEX.with(|re| {
        re.captures(&raw).map(|cap| cap.get(1).unwrap().as_str().to_string())
    });

    let message = raw.clone();

    ParsedLine {
        level,
        timestamp,
        message,
        raw,
    }
}
