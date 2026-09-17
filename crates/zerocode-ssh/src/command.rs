use zerocode_core::{PtyCwd, PtySpec};

use crate::ConnectError;

pub(crate) struct EncodedRemoteCommand<'a> {
    pub shell: String,
    pub environment: Vec<(&'a str, &'a str)>,
}

/// Encode one `PtySpec::remote` as a POSIX shell command.
///
/// SSH's exec request carries one string, not an argv array. Every data value
/// in that string is therefore single-quoted. Nonempty environment values stay
/// out of the command and travel through acknowledged SSH environment requests.
/// Empty values become `unset`, matching the local PTY contract instead of
/// setting an empty value.
pub(crate) fn encode_remote_command(
    spec: &PtySpec,
) -> Result<EncodedRemoteCommand<'_>, ConnectError> {
    let Some(PtyCwd::Remote(cwd)) = spec.cwd.as_ref() else {
        return Err(ConnectError::RemotePtyWorkingDirectoryRequired);
    };
    if spec.program.is_empty() || contains_nul(&spec.program) {
        return Err(ConnectError::InvalidPtyCommand);
    }
    if spec.args.iter().any(|argument| contains_nul(argument)) {
        return Err(ConnectError::InvalidPtyCommand);
    }
    let effective_environment = effective_environment(&spec.env)?;

    let mut command = String::from("cd ");
    push_posix_word(&mut command, cwd.as_str());
    for (name, value) in &effective_environment {
        if value.is_empty() {
            command.push_str(" && unset ");
            command.push_str(name);
        }
    }
    command.push_str(" && exec ");
    push_posix_word(&mut command, &spec.program);
    for argument in &spec.args {
        command.push(' ');
        push_posix_word(&mut command, argument);
    }
    Ok(EncodedRemoteCommand {
        shell: command,
        environment: effective_environment
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .collect(),
    })
}

fn effective_environment(
    environment: &[(String, String)],
) -> Result<Vec<(&str, &str)>, ConnectError> {
    let mut effective = Vec::with_capacity(environment.len());
    for (name, value) in environment {
        if !is_posix_name(name) || contains_nul(value) {
            return Err(ConnectError::InvalidPtyCommand);
        }
        if let Some(previous) = effective.iter().position(|(previous, _)| *previous == name) {
            effective.remove(previous);
        }
        effective.push((name.as_str(), value.as_str()));
    }
    Ok(effective)
}

fn contains_nul(value: &str) -> bool {
    value.as_bytes().contains(&0)
}

fn is_posix_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn push_posix_word(command: &mut String, value: &str) {
    command.push('\'');
    for character in value.chars() {
        if character == '\'' {
            command.push_str("'\"'\"'");
        } else {
            command.push(character);
        }
    }
    command.push('\'');
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::RemotePath;

    fn remote_spec(program: &str, args: &[String], env: &[(String, String)]) -> PtySpec {
        PtySpec::remote(
            program,
            args,
            RemotePath::parse("/srv/work dir").expect("valid remote path"),
            env,
            24,
            80,
        )
    }

    #[test]
    fn quotes_every_data_word_and_unsets_empty_environment_values() {
        let spec = remote_spec(
            "tool name",
            &[
                "a'b".to_owned(),
                "$(not-a-command)".to_owned(),
                String::new(),
            ],
            &[
                ("REMOVE_ME".to_owned(), String::new()),
                ("KEEP".to_owned(), "x'y".to_owned()),
            ],
        );

        let encoded = encode_remote_command(&spec).expect("encodable command");
        assert_eq!(
            encoded.shell,
            "cd '/srv/work dir' && unset REMOVE_ME && exec \
             'tool name' 'a'\"'\"'b' '$(not-a-command)' ''"
        );
        assert_eq!(encoded.environment, vec![("KEEP", "x'y")]);
    }

    #[test]
    fn duplicate_environment_names_use_only_the_last_value() {
        let spec = remote_spec(
            "tool",
            &[],
            &[
                ("SET_LAST".to_owned(), String::new()),
                ("UNSET_LAST".to_owned(), "first".to_owned()),
                ("SET_LAST".to_owned(), "final".to_owned()),
                ("UNSET_LAST".to_owned(), String::new()),
            ],
        );

        let encoded = encode_remote_command(&spec).expect("encodable command");
        assert_eq!(
            encoded.shell,
            "cd '/srv/work dir' && unset UNSET_LAST && exec 'tool'"
        );
        assert_eq!(encoded.environment, vec![("SET_LAST", "final")]);
    }

    #[test]
    fn refuses_inheritance_local_paths_invalid_names_and_nul() {
        let inherited = PtySpec::new("tool", &[], None, &[], 24, 80);
        assert!(matches!(
            encode_remote_command(&inherited),
            Err(ConnectError::RemotePtyWorkingDirectoryRequired)
        ));

        let invalid_name = remote_spec("tool", &[], &[("BAD-NAME".to_owned(), "x".to_owned())]);
        assert!(matches!(
            encode_remote_command(&invalid_name),
            Err(ConnectError::InvalidPtyCommand)
        ));

        let nul = remote_spec("tool", &["bad\0arg".to_owned()], &[]);
        assert!(matches!(
            encode_remote_command(&nul),
            Err(ConnectError::InvalidPtyCommand)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn encoded_words_cannot_inject_the_posix_shell_and_empty_env_is_unset() {
        let danger = "'$(printf INJECTED); printf ALSO_INJECTED";
        let argument = "'; printf ARG_INJECTED; #";
        let script = concat!(
            "if [ \"${REMOVE_ME+x}\" = x ]; then state=set; else state=unset; fi; ",
            "printf '%s|%s|%s' \"$DANGER\" \"$1\" \"$state\""
        );
        let spec = PtySpec::remote(
            "/bin/sh",
            &[
                "-c".to_owned(),
                script.to_owned(),
                "argv0".to_owned(),
                argument.to_owned(),
            ],
            RemotePath::parse("/tmp").expect("valid remote path"),
            &[
                ("DANGER".to_owned(), danger.to_owned()),
                ("REMOVE_ME".to_owned(), String::new()),
            ],
            24,
            80,
        );
        let encoded = encode_remote_command(&spec).expect("encodable command");

        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(encoded.shell)
            .envs(encoded.environment)
            .env("REMOVE_ME", "present")
            .output()
            .expect("run POSIX shell regression");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("UTF-8 fixture output"),
            format!("{danger}|{argument}|unset")
        );
    }
}
