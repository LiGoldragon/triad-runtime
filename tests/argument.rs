use tempfile::TempDir;
use triad_runtime::{ArgumentError, ComponentArgument, ComponentCommand};

#[test]
fn command_classifies_inline_dotos_text_for_text_clients() {
    let command = ComponentCommand::from_arguments(["(Record [payload])"]);

    let argument = command.dotos_argument().expect("dotos argument");

    assert_eq!(
        argument.into_inline_dotos().expect("inline dotos").as_str(),
        "(Record [payload])"
    );
}

#[test]
fn command_classifies_existing_file_as_dotos_file_for_text_clients() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("input.dotos");
    std::fs::write(&path, "(Record [payload])").expect("write input");
    let command = ComponentCommand::from_arguments([path.display().to_string()]);

    let argument = command.dotos_argument().expect("dotos file argument");

    assert_eq!(
        argument.into_dotos_file().expect("dotos file").as_path(),
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
        .dotos_argument()
        .expect_err("missing argument");
    let multiple = ComponentCommand::from_arguments(["one", "two"])
        .dotos_argument()
        .expect_err("multiple arguments");

    assert!(matches!(missing, ArgumentError::ArgumentCount { count: 0 }));
    assert!(matches!(
        multiple,
        ArgumentError::ArgumentCount { count: 2 }
    ));
}

#[test]
fn daemon_argument_rejects_inline_text() {
    let command = ComponentCommand::from_arguments(["(BindingSurface ...)"]);

    let error = command
        .signal_file_argument()
        .expect_err("daemon expects a signal file");

    assert!(matches!(error, ArgumentError::ExpectedSignalFile));
}

#[test]
fn daemon_argument_rejects_dotos_file() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("configuration.dotos");
    std::fs::write(&path, "(BindingSurface)").expect("write input");
    let command = ComponentCommand::from_arguments([path.display().to_string()]);

    let error = command
        .signal_file_argument()
        .expect_err("daemon rejects a DOTOS file path");

    assert!(matches!(error, ArgumentError::ExpectedSignalFile));
}

#[test]
fn pretty_flag_is_recognized_and_removed_from_the_dotos_operand() {
    let plain = ComponentCommand::from_arguments(["(Record [payload])"]);
    assert!(!plain.pretty_requested());

    let pretty = ComponentCommand::from_arguments(["--pretty", "(Record [payload])"]);
    assert!(pretty.pretty_requested());
    assert_eq!(pretty.argument_count(), 1);
    assert_eq!(
        pretty
            .dotos_argument()
            .expect("dotos argument")
            .into_inline_dotos()
            .expect("inline dotos")
            .as_str(),
        "(Record [payload])"
    );
}

#[test]
fn pretty_flag_does_not_relax_the_single_argument_rule() {
    let error = ComponentCommand::from_arguments(["--pretty", "one", "two"])
        .dotos_argument()
        .expect_err("two operands remain an error even with --pretty");

    assert!(matches!(error, ArgumentError::ArgumentCount { count: 2 }));
}

#[test]
fn component_argument_variants_are_distinct() {
    let command = ComponentCommand::from_arguments(["(Record [payload])"]);

    let argument = command.dotos_argument().expect("dotos argument");

    assert!(matches!(argument, ComponentArgument::InlineDotos(_)));
}
