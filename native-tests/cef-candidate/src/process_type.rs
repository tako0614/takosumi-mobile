//! The Chromium process type of a command line, for the guard's forced-cleanup
//! diagnostic: only the `--type=` and `--utility-sub-type=` values, never the
//! rest of the command line, which can hold paths. Pure and platform
//! independent; the Windows guard's self-tests run `tests::all`, and the file
//! also checks alone with `rustc --edition 2021 --test process_type.rs`.

/// The two tokens of a command line, each only when present. A value outside
/// `[A-Za-z0-9._-]{1,64}` reads as `INVALID`, so nothing else can leak.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ProcessType {
    pub(crate) kind: Option<String>,
    pub(crate) utility_sub_type: Option<String>,
}

pub(crate) const INVALID: &str = "invalid";

/// Split a Windows command line into its arguments as the Microsoft C runtime
/// does. The program name (the first argument) ends at whitespace outside
/// quotes, a quote toggles quoting and backslashes are literal. After it:
/// whitespace outside quotes separates; 2n backslashes before a quote are n
/// backslashes and the quote toggles quoting; 2n+1 are n and a literal quote;
/// `""` inside quotes is a literal quote; other backslashes are literal.
pub(crate) fn windows_arguments(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut arguments = Vec::new();
    let mut index = 0;
    // The program name.
    let mut program = String::new();
    let mut quoted = false;
    while index < chars.len() {
        let c = chars[index];
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && (c == ' ' || c == '\t') {
            break;
        } else {
            program.push(c);
        }
        index += 1;
    }
    arguments.push(program);
    loop {
        while index < chars.len() && (chars[index] == ' ' || chars[index] == '\t') {
            index += 1;
        }
        if index >= chars.len() {
            return arguments;
        }
        let mut argument = String::new();
        let mut quoted = false;
        while index < chars.len() {
            let c = chars[index];
            if c == '\\' {
                let start = index;
                while index < chars.len() && chars[index] == '\\' {
                    index += 1;
                }
                let count = index - start;
                if index < chars.len() && chars[index] == '"' {
                    argument.extend(std::iter::repeat_n('\\', count / 2));
                    if count % 2 == 1 {
                        argument.push('"');
                        index += 1;
                    }
                } else {
                    argument.extend(std::iter::repeat_n('\\', count));
                }
                continue;
            }
            if c == '"' {
                if quoted && chars.get(index + 1) == Some(&'"') {
                    argument.push('"');
                    index += 2;
                    continue;
                }
                quoted = !quoted;
                index += 1;
                continue;
            }
            if !quoted && (c == ' ' || c == '\t') {
                break;
            }
            argument.push(c);
            index += 1;
        }
        arguments.push(argument);
    }
}

/// The process type tokens of a split command line (the program name first,
/// as /proc/<pid>/cmdline and `windows_arguments` give it). Switches end at a
/// bare `--`, as in Chromium; a repeated switch keeps its last value.
pub(crate) fn process_type<'a>(arguments: impl IntoIterator<Item = &'a str>) -> ProcessType {
    let mut found = ProcessType::default();
    for argument in arguments.into_iter().skip(1) {
        if argument == "--" {
            break;
        }
        if let Some(value) = argument.strip_prefix("--type=") {
            found.kind = Some(token(value));
        } else if let Some(value) = argument.strip_prefix("--utility-sub-type=") {
            found.utility_sub_type = Some(token(value));
        }
    }
    found
}

fn token(value: &str) -> String {
    let safe = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if safe {
        value.to_owned()
    } else {
        INVALID.to_owned()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn of(line: &str) -> ProcessType {
        let arguments = windows_arguments(line);
        process_type(arguments.iter().map(String::as_str))
    }

    fn kind(kind: &str, sub: Option<&str>) -> ProcessType {
        ProcessType {
            kind: Some(kind.to_owned()),
            utility_sub_type: sub.map(str::to_owned),
        }
    }

    pub(crate) fn windows_quoting() {
        assert_eq!(
            windows_arguments(r#""C:\Program Files\a b\x.exe" --type=renderer"#),
            [r"C:\Program Files\a b\x.exe", "--type=renderer"]
        );
        assert_eq!(windows_arguments(r#"x.exe  a	b "#), ["x.exe", "a", "b"]);
        assert_eq!(
            windows_arguments(r#"x.exe "a b" c\d "e\"f" g\\"h i" "j""k""#),
            ["x.exe", "a b", r"c\d", r#"e"f"#, r"g\h i", r#"j"k"#]
        );
        // A quoted program name keeps its backslashes, even before the quote.
        assert_eq!(
            windows_arguments(r#""C:\dir\"x.exe y"#),
            [r"C:\dir\x.exe", "y"]
        );
        assert_eq!(windows_arguments(""), [""]);
    }

    pub(crate) fn chromium_command_lines() {
        assert_eq!(
            of(
                r#""C:\Users\runner admin\x.exe" --type=renderer --user-data-dir="C:\a b\--type=gpu-process" --lang=en-US"#
            ),
            kind("renderer", None)
        );
        assert_eq!(
            of(
                r#"x.exe --type=utility "--utility-sub-type=network.mojom.NetworkService" --service-sandbox-type=none"#
            ),
            kind("utility", Some("network.mojom.NetworkService"))
        );
        assert_eq!(
            of("x.exe --type=utility --utility-sub-type=storage.mojom.StorageService"),
            kind("utility", Some("storage.mojom.StorageService"))
        );
        // The browser process and the console host have no type.
        assert_eq!(
            of(r#""C:\a b\x.exe" --no-sandbox --lang=en-US"#),
            ProcessType::default()
        );
        assert_eq!(
            of(r"\??\C:\Windows\system32\conhost.exe 0xffffffff -ForceV1"),
            ProcessType::default()
        );
        // Only the switch itself counts: not the program, not a value that
        // contains it, nothing after a bare `--`.
        assert_eq!(of("--type=gpu-process x"), ProcessType::default());
        assert_eq!(of("x.exe --x=--type=gpu-process"), ProcessType::default());
        assert_eq!(of("x.exe -- --type=renderer"), ProcessType::default());
        assert_eq!(
            of("x.exe --type=gpu-process --type=renderer"),
            kind("renderer", None)
        );
        // A value with anything but [A-Za-z0-9._-] (or too long) never leaks.
        assert_eq!(
            of(r#"x.exe "--type=C:\Users\a b" --utility-sub-type="#),
            ProcessType {
                kind: Some(INVALID.to_owned()),
                utility_sub_type: Some(INVALID.to_owned()),
            }
        );
        assert_eq!(
            of(&format!("x.exe --type={}", "a".repeat(65))),
            kind(INVALID, None)
        );
        // /proc/<pid>/cmdline arguments are already split.
        assert_eq!(
            process_type(["/opt/x", "--type=zygote", "--no-zygote-sandbox"]),
            kind("zygote", None)
        );
    }

    pub(crate) fn all() {
        windows_quoting();
        chromium_command_lines();
    }

    #[test]
    fn command_lines() {
        all();
    }
}
