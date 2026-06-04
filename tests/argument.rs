use tempfile::TempDir;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

#[test]
fn command_classifies_inline_nota_text_for_text_clients() {
    let command = ComponentCommand::from_arguments(["(Record [payload])"]);

    let argument = command.nota_argument().expect("nota argument");

    assert_eq!(
        argument.into_inline_nota().expect("inline nota").as_str(),
        "(Record [payload])"
    );
}

#[test]
fn command_classifies_existing_file_as_nota_file_for_text_clients() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("input.nota");
    std::fs::write(&path, "(Record [payload])").expect("write input");
    let command = ComponentCommand::from_arguments([path.display().to_string()]);

    let argument = command.nota_argument().expect("nota file argument");

    assert_eq!(
        argument.into_nota_file().expect("nota file").as_path(),
        path.as_path()
    );
}

#[test]
fn command_classifies_existing_file_as_signal_file_for_daemons() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("configuration.rkyv");
    std::fs::write(&path, [1, 2, 3]).expect("write input");
    let command = ComponentCommand::from_arguments([path.display().to_string()]);

    let argument = command
        .signal_file_argument()
        .expect("signal file argument");

    assert_eq!(
        argument.into_signal_file().expect("signal file").as_path(),
        path.as_path()
    );
}

#[test]
fn command_rejects_zero_or_multiple_arguments() {
    let missing = ComponentCommand::from_arguments(Vec::<String>::new())
        .nota_argument()
        .expect_err("missing argument");
    let multiple = ComponentCommand::from_arguments(["one", "two"])
        .nota_argument()
        .expect_err("multiple arguments");

    assert!(matches!(missing, ArgumentError::ArgumentCount { count: 0 }));
    assert!(matches!(
        multiple,
        ArgumentError::ArgumentCount { count: 2 }
    ));
}

#[test]
fn daemon_argument_rejects_inline_text() {
    let command = ComponentCommand::from_arguments(["(DaemonConfiguration ...)"]);

    let error = command
        .signal_file_argument()
        .expect_err("daemon expects a signal file");

    assert!(matches!(error, ArgumentError::ExpectedSignalFile));
}

#[test]
fn component_argument_variants_are_distinct() {
    let command = ComponentCommand::from_arguments(["(Record [payload])"]);

    let argument = command.nota_argument().expect("nota argument");

    assert!(matches!(argument, ComponentArgument::InlineNota(_)));
}
