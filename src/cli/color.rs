use std::io::{self, IsTerminal};

use clap::ValueEnum;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl std::fmt::Display for ColorMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            ColorMode::Auto => "auto",
            ColorMode::Always => "always",
            ColorMode::Never => "never",
        };

        formatter.write_str(value)
    }
}

#[derive(Debug)]
pub struct Colors {
    enabled: bool,
}

impl Colors {
    pub fn new(mode: ColorMode) -> Self {
        let enabled = match mode {
            ColorMode::Auto => io::stdout().is_terminal(),
            ColorMode::Always => true,
            ColorMode::Never => false,
        };

        Self { enabled }
    }

    pub fn bold(&self, text: impl AsRef<str>) -> String {
        self.paint(text.as_ref(), "1")
    }

    pub fn dim(&self, text: impl AsRef<str>) -> String {
        self.paint(text.as_ref(), "2")
    }

    pub fn red(&self, text: impl AsRef<str>) -> String {
        self.paint(text.as_ref(), "31")
    }

    pub fn green(&self, text: impl AsRef<str>) -> String {
        self.paint(text.as_ref(), "32")
    }

    pub fn cyan(&self, text: impl AsRef<str>) -> String {
        self.paint(text.as_ref(), "36")
    }

    fn paint(&self, text: &str, code: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}
