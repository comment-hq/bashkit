//! echo builtin command

use async_trait::async_trait;

use super::{Builtin, BuiltinHelper, Context};
use crate::error::Result;
use crate::interpreter::ExecResult;

/// The echo builtin command.
pub struct Echo;

impl BuiltinHelper for Echo {
    const NAME: &'static str = "echo";
}

#[async_trait]
impl Builtin for Echo {
    async fn execute(&self, ctx: Context<'_>) -> Result<ExecResult> {
        if let Some(r) = Self::check_help(
            ctx.args,
            "Usage: echo [SHORT-OPTION]... [STRING]...\n  or:  echo LONG-OPTION\nEcho the STRING(s) to standard output.\n\n  -n\tdo not output the trailing newline\n  -e\tenable interpretation of backslash escapes\n  -E\tdisable interpretation of backslash escapes (default)\n  --help\tdisplay this help and exit\n  --version\toutput version information and exit\n",
            Some("echo (bashkit) 0.1"),
        ) {
            return Ok(r);
        }
        let mut output = Vec::new();
        let mut add_newline = true;
        let mut interpret_escapes = false;
        let mut args_iter = ctx.args.iter().peekable();

        // Parse options - support combined flags like -en, -ne, -neE
        while let Some(arg) = args_iter.peek() {
            let arg_str = arg.as_str();
            if arg_str.starts_with('-') && arg_str.len() > 1 && !arg_str.starts_with("--") {
                let mut is_valid_option = true;
                // Check if all characters after '-' are valid options
                for c in arg_str[1..].chars() {
                    if !matches!(c, 'n' | 'e' | 'E') {
                        is_valid_option = false;
                        break;
                    }
                }
                if is_valid_option {
                    // Process each flag character
                    for c in arg_str[1..].chars() {
                        match c {
                            'n' => add_newline = false,
                            'e' => interpret_escapes = true,
                            'E' => interpret_escapes = false,
                            _ => {}
                        }
                    }
                    args_iter.next();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Collect remaining arguments
        let remaining: Vec<&String> = args_iter.collect();

        for (i, arg) in remaining.iter().enumerate() {
            if i > 0 {
                output.push(b' ');
            }

            if interpret_escapes {
                output.extend_from_slice(&interpret_escape_sequences(arg));
            } else {
                output.extend_from_slice(arg.as_bytes());
            }
        }

        if add_newline {
            output.push(b'\n');
        }

        Ok(ExecResult::ok_bytes(output))
    }
}

/// Expand `echo -e` escapes. `\0nnn` and `\xHH` are single bytes, as in bash,
/// even when they do not form valid UTF-8.
fn interpret_escape_sequences(s: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push(b'\n'),
                Some('t') => result.push(b'\t'),
                Some('r') => result.push(b'\r'),
                Some('\\') => result.push(b'\\'),
                Some('a') => result.push(0x07), // bell
                Some('b') => result.push(0x08), // backspace
                Some('f') => result.push(0x0c), // form feed
                Some('v') => result.push(0x0b), // vertical tab
                Some('0') => {
                    // Octal escape \0nnn
                    let mut value = 0u8;
                    for _ in 0..3 {
                        if let Some(&digit) = chars.peek() {
                            if ('0'..='7').contains(&digit) {
                                value = value * 8 + (digit as u8 - b'0');
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    result.push(value);
                }
                Some('x') => {
                    // Hex escape \xHH
                    let mut value = 0u8;
                    for _ in 0..2 {
                        if let Some(&digit) = chars.peek() {
                            if digit.is_ascii_hexdigit() {
                                value = value * 16
                                    + digit.to_digit(16).expect(
                                        "to_digit(16) valid: guarded by is_ascii_hexdigit()",
                                    ) as u8;
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    result.push(value);
                }
                Some('c') => {
                    // Stop output
                    break;
                }
                Some(other) => {
                    result.push(b'\\');
                    push_char(&mut result, other);
                }
                None => result.push(b'\\'),
            }
        } else {
            push_char(&mut result, ch);
        }
    }

    result
}

fn push_char(out: &mut Vec<u8>, ch: char) {
    let mut buf = [0; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_sequences() {
        assert_eq!(interpret_escape_sequences("hello\\nworld"), b"hello\nworld");
        assert_eq!(interpret_escape_sequences("tab\\there"), b"tab\there");
        assert_eq!(interpret_escape_sequences("\\\\backslash"), b"\\backslash");
        assert_eq!(
            interpret_escape_sequences("\\xff\\0376é"),
            b"\xff\xfe\xc3\xa9"
        );
    }
}
