//! Reminders for the Mac runner.
//!
//! Like the Kotlin original, a reminder fires from an in-process timer while `ios-runner` keeps
//! running, prints its line, and shows a macOS notification through `osascript`. The text travels
//! as an argument of an `on run argv` script, never spliced into the script itself.

use std::io::Write;
use std::sync::{Arc, Mutex};

use chrono::TimeZone;
use xiaoman_api::ReminderScheduler;

type Output = Arc<Mutex<Box<dyn Write + Send>>>;

/// Accepts reminders and prints when one was scheduled.
pub struct MacReminders {
    out: Output,
}

impl MacReminders {
    pub fn new(out: Output) -> Self {
        Self { out }
    }

    pub fn line(&self, text: &str) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{text}");
            let _ = out.flush();
        }
    }
}

impl ReminderScheduler for MacReminders {
    fn schedule(&self, text: &str, at_millis: i64) {
        self.line(&format!(
            "已安排提醒（{}）：{}。提醒只在 ios-runner 运行期间有效。",
            format_time(at_millis),
            text
        ));
        let out = self.out.clone();
        let text = text.to_string();
        std::thread::spawn(move || {
            // The runner lives for the session, so one sleeping thread per reminder is enough; a
            // stopped runner drops its reminders, exactly like the Kotlin in-process timer.
            let wait = at_millis.saturating_sub(xiaoman_api::now_millis());
            if wait > 0 {
                std::thread::sleep(std::time::Duration::from_millis(wait as u64));
            }
            if let Ok(mut out) = out.lock() {
                let _ = writeln!(out, "提醒：{text}");
                let _ = out.flush();
            }
            notify(&text);
        });
    }
}

/// `osascript -e 'on run argv …' -- <text>`, the way the Kotlin `MacReminders` showed it: the
/// reminder text is an argument, so a quote or a newline in it cannot change the script.
const NOTIFICATION_SCRIPT: &str =
    "on run argv\n\tdisplay notification (item 1 of argv) with title \"小满\"\nend run";

fn notification_args(text: &str) -> [String; 3] {
    [
        "-e".to_string(),
        NOTIFICATION_SCRIPT.to_string(),
        text.to_string(),
    ]
}

fn notify(text: &str) {
    if !cfg!(target_os = "macos") || !std::path::Path::new("/usr/bin/osascript").exists() {
        return;
    }
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args(notification_args(text))
        .status();
}

fn format_time(at_millis: i64) -> String {
    match chrono::Local.timestamp_millis_opt(at_millis).single() {
        Some(time) => time.format("%m-%d %H:%M").to_string(),
        None => at_millis.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_an_argument_not_part_of_the_script() {
        let text = "喝水 \"现在\"\n并休息";
        let args = notification_args(text);
        assert_eq!("-e", args[0]);
        assert!(
            !args[1].contains(text),
            "the script never holds the text: {}",
            args[1]
        );
        assert!(args[1].contains("on run argv"), "{}", args[1]);
        assert_eq!(text, args[2]);
    }
}
