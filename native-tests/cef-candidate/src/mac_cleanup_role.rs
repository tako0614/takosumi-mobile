//! Diagnostic-only decoding of 64-bit Darwin KERN_PROCARGS2. Return only a
//! closed role, never executable paths, arbitrary argv or environment values.
//! XNU pads the saved executable path to a pointer boundary before argv. Its
//! stripped `executable_path=` prefix is 16 bytes, also aligned to 8. Skipping
//! all NULs instead would lose an empty argv[0] and could consume environment.
//! The caller must reject incomplete sysctl output before using this decoder.

fn string<'a>(bytes: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    let end = bytes.get(*at..)?.iter().position(|byte| *byte == 0)? + *at;
    let value = &bytes[*at..end];
    *at = end + 1;
    Some(value)
}

pub(crate) fn parse(bytes: &[u8]) -> Option<&'static str> {
    let argc = i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?);
    if !(1..=4096).contains(&argc) {
        return None;
    }
    let mut at = 4;
    let path = string(bytes, &mut at)?;
    if path.is_empty() {
        return None;
    }
    let padding = (8 - (path.len() + 1) % 8) % 8;
    if bytes.get(at..at + padding)?.iter().any(|byte| *byte != 0) {
        return None;
    }
    at += padding;
    let mut role = "none";
    let mut switches = true;
    for index in 0..argc {
        let argument = string(bytes, &mut at)?;
        if index == 0 {
            if argument.is_empty() {
                return None;
            }
            continue;
        }
        if argument == b"--" {
            switches = false;
        }
        if switches {
            if let Some(kind) = argument.strip_prefix(b"--type=") {
                role = if kind == b"code-sign-clone-cleanup" {
                    "code-sign-clone-cleanup"
                } else {
                    "other"
                };
            }
        }
    }
    Some(role)
}

#[cfg(test)]
// The guard has a custom harness, so these run separately with rustc --test
// (CI requires all four). Its harness build otherwise sees unused test code.
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    fn packet(path: &[u8], arguments: &[&[u8]], environment: &[u8]) -> Vec<u8> {
        let mut bytes = (arguments.len() as i32).to_ne_bytes().to_vec();
        bytes.extend_from_slice(path);
        bytes.push(0);
        bytes.resize(4 + (path.len() + 1).next_multiple_of(8), 0);
        for argument in arguments {
            bytes.extend_from_slice(argument);
            bytes.push(0);
        }
        bytes.extend_from_slice(environment);
        bytes
    }

    #[test]
    fn exact_padding_preserves_arguments_and_excludes_environment() {
        for length in 1..=16 {
            let path = vec![b'x'; length];
            assert_eq!(
                parse(&packet(
                    &path,
                    &[b"helper", b"--type=code-sign-clone-cleanup"],
                    b"secret=private\0"
                )),
                Some("code-sign-clone-cleanup")
            );
            assert_eq!(
                parse(&packet(
                    &path,
                    &[b"helper", b""],
                    b"--type=code-sign-clone-cleanup\0"
                )),
                Some("none")
            );
        }
    }

    #[test]
    fn empty_argv_zero_cannot_shift_environment_into_arguments() {
        assert_eq!(
            parse(&packet(
                b"/helper",
                &[b"", b""],
                b"--type=code-sign-clone-cleanup\0X=private\0"
            )),
            None
        );
        assert_eq!(
            parse(&packet(
                b"/helper",
                &[b"--type=code-sign-clone-cleanup"],
                b""
            )),
            Some("none")
        );
    }

    #[test]
    fn malformed_or_incomplete_output_is_unknown() {
        let good = packet(
            b"/helper",
            &[b"helper", b"--type=code-sign-clone-cleanup"],
            b"",
        );
        for end in 0..good.len() {
            assert_eq!(parse(&good[..end]), None, "prefix length {end}");
        }
        for count in [-1_i32, 0, 4097, i32::MAX] {
            let mut bad = good.clone();
            bad[..4].copy_from_slice(&count.to_ne_bytes());
            assert_eq!(parse(&bad), None);
        }
        let mut bad = packet(b"a", &[b"helper"], b"");
        bad[6] = b'x'; // first alignment byte after path + NUL
        assert_eq!(parse(&bad), None);
    }

    #[test]
    fn only_exact_type_before_separator_with_last_switch_semantics() {
        for (arguments, expected) in [
            (
                vec![
                    b"helper".as_slice(),
                    b"--type=code-sign-clone-cleanup",
                    b"--type=renderer",
                ],
                "other",
            ),
            (
                vec![
                    b"helper".as_slice(),
                    b"--type=renderer",
                    b"--type=code-sign-clone-cleanup",
                ],
                "code-sign-clone-cleanup",
            ),
            (
                vec![
                    b"helper".as_slice(),
                    b"--",
                    b"--type=code-sign-clone-cleanup",
                ],
                "none",
            ),
            (
                vec![
                    b"helper".as_slice(),
                    b"--type=code-sign-clone-cleanup-private",
                ],
                "other",
            ),
            (
                vec![
                    b"helper".as_slice(),
                    b"--type=https://private.example/?secret",
                ],
                "other",
            ),
            (vec![b"helper".as_slice(), b"--type=\xff"], "other"),
        ] {
            assert_eq!(parse(&packet(b"/helper", &arguments, b"")), Some(expected));
        }
    }
}
