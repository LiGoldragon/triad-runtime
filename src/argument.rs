use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentCommand {
    arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentArgument {
    InlineDotos(InlineDotos),
    DotosFile(DotosFile),
    SignalFile(SignalFile),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RawArgument {
    InlineText(InlineText),
    FilePath(ArgumentFilePath),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineDotos {
    text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InlineText {
    text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DotosFile {
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalFile {
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArgumentFilePath {
    path: PathBuf,
}

#[derive(Debug, Error)]
pub enum ArgumentError {
    #[error("expected exactly one component argument, received {count}")]
    ArgumentCount { count: usize },

    #[error("expected a signal-encoded file path")]
    ExpectedSignalFile,
}

impl ComponentCommand {
    pub fn from_environment() -> Self {
        Self::from_arguments(std::env::args().skip(1))
    }

    pub fn from_arguments<Arguments, Argument>(arguments: Arguments) -> Self
    where
        Arguments: IntoIterator<Item = Argument>,
        Argument: Into<String>,
    {
        Self {
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }

    /// The `--pretty` flag every DOTOS-printing CLI honors: a request to reflow
    /// the reply for reading. Parsing it here keeps the flag out of each CLI's
    /// operand handling and adds no DOTOS dependency to the shared harness, so a
    /// daemon build that never enables `dotos-text` still carries no DOTOS code.
    const PRETTY_FLAG: &'static str = "--pretty";

    /// Whether the caller asked for readable DOTOS output with `--pretty`.
    ///
    /// A CLI passes this to `dotos::DotosOutputForm::from_pretty_requested` at its
    /// print site; the daemon paths ignore it.
    pub fn pretty_requested(&self) -> bool {
        self.arguments
            .iter()
            .any(|argument| argument == Self::PRETTY_FLAG)
    }

    pub fn argument_count(&self) -> usize {
        self.operands().count()
    }

    /// The positional arguments, with any recognized flag such as `--pretty`
    /// removed, so the single-argument rule counts only real DOTOS operands.
    fn operands(&self) -> impl Iterator<Item = &String> {
        self.arguments
            .iter()
            .filter(|argument| *argument != Self::PRETTY_FLAG)
    }

    pub fn dotos_argument(&self) -> Result<ComponentArgument, ArgumentError> {
        match self.raw_argument()? {
            RawArgument::InlineText(text) => Ok(ComponentArgument::InlineDotos(InlineDotos::new(
                text.into_text(),
            ))),
            RawArgument::FilePath(path) => Ok(ComponentArgument::DotosFile(DotosFile::new(
                path.into_path(),
            ))),
        }
    }

    pub fn signal_file_argument(&self) -> Result<ComponentArgument, ArgumentError> {
        match self.raw_argument()? {
            RawArgument::InlineText(_) => Err(ArgumentError::ExpectedSignalFile),
            RawArgument::FilePath(path) => {
                if path.is_dotos_file() {
                    return Err(ArgumentError::ExpectedSignalFile);
                }
                Ok(ComponentArgument::SignalFile(SignalFile::new(
                    path.into_path(),
                )))
            }
        }
    }

    fn raw_argument(&self) -> Result<RawArgument, ArgumentError> {
        let operands = self.operands().collect::<Vec<_>>();
        match operands.as_slice() {
            [argument] => Ok(RawArgument::from_single(argument)),
            _ => Err(ArgumentError::ArgumentCount {
                count: operands.len(),
            }),
        }
    }
}

impl ComponentArgument {
    pub fn into_inline_dotos(self) -> Option<InlineDotos> {
        match self {
            Self::InlineDotos(argument) => Some(argument),
            Self::DotosFile(_) | Self::SignalFile(_) => None,
        }
    }

    pub fn into_dotos_file(self) -> Option<DotosFile> {
        match self {
            Self::DotosFile(argument) => Some(argument),
            Self::InlineDotos(_) | Self::SignalFile(_) => None,
        }
    }

    pub fn into_signal_file(self) -> Option<SignalFile> {
        match self {
            Self::SignalFile(argument) => Some(argument),
            Self::InlineDotos(_) | Self::DotosFile(_) => None,
        }
    }
}

impl RawArgument {
    fn from_single(argument: &str) -> Self {
        if Path::new(argument).exists() {
            Self::FilePath(ArgumentFilePath::new(argument))
        } else {
            Self::InlineText(InlineText::new(argument))
        }
    }
}

impl InlineDotos {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn into_string(self) -> String {
        self.text
    }
}

impl InlineText {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    fn into_text(self) -> String {
        self.text
    }
}

impl DotosFile {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

impl SignalFile {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

impl ArgumentFilePath {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn is_dotos_file(&self) -> bool {
        self.path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "dotos")
    }

    fn into_path(self) -> PathBuf {
        self.path
    }
}
