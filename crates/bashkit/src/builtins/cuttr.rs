//! Cut and tr builtins - extract fields and translate characters

use async_trait::async_trait;

use super::{Builtin, BuiltinHelper, Context, read_text_file};
use crate::error::Result;
use crate::fs::vfs_join;
use crate::interpreter::ExecResult;

// Keep tr set expansion bounded before stdin/output controls can apply.
const MAX_TR_EXPANDED_SET_CHARS: usize = 4096;

/// The cut builtin - remove sections from each line.
///
/// Usage: cut -d DELIM -f FIELDS [FILE...]
///        cut -b BYTES [FILE...]
///        cut -c CHARS [FILE...]
///
/// Options:
///   -d DELIM            Use DELIM instead of TAB for field delimiter
///   -f FIELDS           Select only these fields (1-indexed)
///   -b BYTES            Select only these bytes (1-indexed, same as -c for ASCII)
///   -c CHARS            Select only these characters (1-indexed)
///   -s                  Only print lines containing delimiter (with -f)
///   --complement        Complement the selection
///   --output-delimiter  Use STRING as output delimiter
pub struct Cut;

impl BuiltinHelper for Cut {
    const NAME: &'static str = "cut";
}

#[derive(PartialEq)]
enum CutMode {
    Fields,
    Chars,
}

#[async_trait]
impl Builtin for Cut {
    async fn execute(&self, ctx: Context<'_>) -> Result<ExecResult> {
        if let Some(r) = Self::check_help(
            ctx.args,
            "Usage: cut OPTION... [FILE]...\nPrint selected parts of lines from each FILE to standard output.\n\n  -d DELIM\t\tuse DELIM instead of TAB for field delimiter\n  -f FIELDS\t\tselect only these fields\n  -b BYTES\t\tselect only these bytes\n  -c CHARS\t\tselect only these characters\n  -s\t\t\tonly print lines containing delimiter\n  -z\t\t\tline delimiter is NUL, not newline\n  --complement\t\tcomplement the selection\n  --output-delimiter=STRING\tuse STRING as output delimiter\n  --help\t\tdisplay this help and exit\n  --version\t\toutput version information and exit\n",
            Some("cut (bashkit) 0.1"),
        ) {
            return Ok(r);
        }
        let mut delimiter = '\t';
        let mut spec = String::new();
        let mut mode = CutMode::Fields;
        let mut complement = false;
        let mut only_delimited = false;
        let mut zero_terminated = false;
        let mut output_delimiter: Option<String> = None;
        let mut files = Vec::new();

        // Parse arguments
        let mut p = super::arg_parser::ArgParser::new(ctx.args);
        while !p.is_done() {
            if let Some(val) = p.flag_value_opt("-d") {
                delimiter = val.chars().next().unwrap_or('\t');
            } else if let Some(val) = p.flag_value_opt("-f") {
                spec = val.to_string();
                mode = CutMode::Fields;
            } else if let Some(val) = p.flag_value_opt("-c") {
                spec = val.to_string();
                mode = CutMode::Chars;
            } else if let Some(val) = p.flag_value_opt("-b") {
                spec = val.to_string();
                mode = CutMode::Chars;
            } else if p.flag("-s") {
                only_delimited = true;
            } else if p.flag("-z") {
                zero_terminated = true;
            } else if p.flag("--complement") {
                complement = true;
            } else if let Some(val) = p
                .current()
                .and_then(|s| s.strip_prefix("--output-delimiter="))
            {
                output_delimiter = Some(val.to_string());
                p.advance();
            } else if p.flag("--output-delimiter") {
                if let Some(val) = p.positional() {
                    output_delimiter = Some(val.to_string());
                }
            } else if let Some(arg) = p.current().filter(|s| !s.starts_with('-')) {
                files.push(arg.to_string());
                p.advance();
            } else if p.current() == Some("-") || p.current() == Some("--") {
                // "-" stdin operand / "--" end-of-options: preserve handling.
                p.advance();
            } else if p.is_flag() {
                // Reject unknown options instead of dropping them silently.
                return Ok(super::invalid_option(
                    "cut",
                    p.current().unwrap_or_default(),
                    1,
                ));
            } else {
                p.advance();
            }
        }

        if spec.is_empty() {
            return Ok(Self::err("you must specify a list of fields", 1));
        }

        // Parse position specification (supports open-ended ranges like "3-" and "-3")
        let positions = parse_position_spec(&spec);
        let out_delim = output_delimiter.unwrap_or_else(|| delimiter.to_string());

        let process_line = |line: &str| -> Option<String> {
            match mode {
                CutMode::Chars => {
                    let chars: Vec<char> = line.chars().collect();
                    let total = chars.len();
                    let resolved = resolve_positions(&positions, total);
                    let selected: Vec<char> = if complement {
                        chars
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| !resolved.contains(&(i + 1)))
                            .map(|(_, c)| *c)
                            .collect()
                    } else {
                        resolved
                            .iter()
                            .filter_map(|&p| chars.get(p - 1).copied())
                            .collect()
                    };
                    Some(selected.into_iter().collect())
                }
                CutMode::Fields => {
                    // -s: skip lines without delimiter
                    if only_delimited && !line.contains(delimiter) {
                        return None;
                    }
                    let parts: Vec<&str> = line.split(delimiter).collect();
                    let total = parts.len();
                    let resolved = resolve_positions(&positions, total);
                    let selected: Vec<&str> = if complement {
                        parts
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| !resolved.contains(&(i + 1)))
                            .map(|(_, s)| *s)
                            .collect()
                    } else {
                        resolved
                            .iter()
                            .filter_map(|&f| parts.get(f - 1).copied())
                            .collect()
                    };
                    Some(selected.join(&out_delim))
                }
            }
        };

        let mut output = String::new();
        let line_sep = if zero_terminated { '\0' } else { '\n' };
        let out_sep = if zero_terminated { "\0" } else { "\n" };

        let process_input = |text: &str, output: &mut String| {
            for line in text.split(line_sep) {
                if line.is_empty() {
                    continue;
                }
                if let Some(result) = process_line(line) {
                    output.push_str(&result);
                    output.push_str(out_sep);
                }
            }
        };

        if files.is_empty() || files.iter().all(|f| f.as_str() == "-") {
            if let Some(stdin) = ctx.stdin {
                process_input(stdin, &mut output);
            }
        } else {
            for file in &files {
                if file.as_str() == "-" {
                    if let Some(stdin) = ctx.stdin {
                        process_input(stdin, &mut output);
                    }
                    continue;
                }

                let path = if file.starts_with('/') {
                    std::path::PathBuf::from(file)
                } else {
                    vfs_join(ctx.cwd, file)
                };

                let text = match read_text_file(&*ctx.fs, &path, "cut").await {
                    Ok(t) => t,
                    Err(e) => return Ok(e),
                };
                process_input(&text, &mut output);
            }
        }

        Ok(ExecResult::ok(output))
    }
}

/// Position in a cut specification — can be open-ended
#[derive(Debug, Clone)]
enum Position {
    Single(usize),
    Range(usize, usize),
    FromStart(usize), // -N (1 to N)
    ToEnd(usize),     // N- (N to end)
}

/// Parse a position specification like "1", "1,3", "1-3", "3-", "-3"
fn parse_position_spec(spec: &str) -> Vec<Position> {
    let mut positions = Vec::new();

    for part in spec.split(',') {
        if let Some((start, end)) = part.split_once('-') {
            if start.is_empty() {
                // -N
                if let Ok(n) = end.parse::<usize>() {
                    positions.push(Position::FromStart(n));
                }
            } else if end.is_empty() {
                // N-
                if let Ok(n) = start.parse::<usize>() {
                    positions.push(Position::ToEnd(n));
                }
            } else {
                // N-M
                let s: usize = start.parse().unwrap_or(1);
                let e: usize = end.parse().unwrap_or(s);
                positions.push(Position::Range(s, e));
            }
        } else if let Ok(f) = part.parse::<usize>()
            && f > 0
        {
            positions.push(Position::Single(f));
        }
    }

    positions
}

/// Resolve position specifications into concrete 1-indexed positions
fn resolve_positions(positions: &[Position], total: usize) -> Vec<usize> {
    let mut result = Vec::new();
    for pos in positions {
        match pos {
            Position::Single(n) => {
                if *n > 0 && *n <= total {
                    result.push(*n);
                }
            }
            Position::Range(s, e) => {
                let start = (*s).max(1);
                let end = (*e).min(total);
                for i in start..=end {
                    result.push(i);
                }
            }
            Position::FromStart(n) => {
                for i in 1..=(*n).min(total) {
                    result.push(i);
                }
            }
            Position::ToEnd(n) => {
                let start = (*n).max(1);
                for i in start..=total {
                    result.push(i);
                }
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

/// The tr builtin - translate or delete characters.
///
/// Usage: tr [-d] [-s] [-c/-C] SET1 [SET2]
///
/// Options:
///   -d     Delete characters in SET1
///   -s     Squeeze repeated output characters in SET2 (or SET1 if no SET2)
///   -c/-C  Complement SET1 (use all chars NOT in SET1)
///
/// SET1 and SET2 can contain character ranges like a-z, A-Z, 0-9
/// and POSIX classes like [:lower:], [:upper:], [:digit:], [:alpha:],
/// [:alnum:], [:space:], [:blank:], [:punct:], [:xdigit:], [:print:], [:graph:]
pub struct Tr;

impl BuiltinHelper for Tr {
    const NAME: &'static str = "tr";
}

#[async_trait]
impl Builtin for Tr {
    async fn execute(&self, ctx: Context<'_>) -> Result<ExecResult> {
        if let Some(r) = Self::check_help(
            ctx.args,
            "Usage: tr [OPTION]... SET1 [SET2]\nTranslate, squeeze, and/or delete characters from standard input.\n\n  -d\t\tdelete characters in SET1\n  -s\t\tsqueeze repeated output characters\n  -c, -C\tcomplement SET1\n  --help\tdisplay this help and exit\n  --version\toutput version information and exit\n",
            Some("tr (bashkit) 0.1"),
        ) {
            return Ok(r);
        }
        let mut delete = false;
        let mut squeeze = false;
        let mut complement = false;

        // Parse flags (can be combined like -ds, -cd)
        let mut non_flag_args: Vec<String> = Vec::new();
        let mut p = super::arg_parser::ArgParser::new(ctx.args);
        while !p.is_done() {
            let flags = p.bool_flags("dscC");
            if !flags.is_empty() {
                for ch in flags {
                    match ch {
                        'd' => delete = true,
                        's' => squeeze = true,
                        'c' | 'C' => complement = true,
                        _ => {}
                    }
                }
            } else if non_flag_args.is_empty() && p.is_flag() && p.current() != Some("--") {
                // Reject unknown options only in the option phase (before any
                // SET is read); once SETs are collected a leading `-` is an operand.
                return Ok(super::invalid_option(
                    "tr",
                    p.current().unwrap_or_default(),
                    1,
                ));
            } else if let Some(arg) = p.positional() {
                non_flag_args.push(arg.to_string());
            }
        }

        if non_flag_args.is_empty() {
            return Ok(Self::err("missing operand", 1));
        }

        let set1 = match expand_char_set(&non_flag_args[0]) {
            Ok(set) => set,
            Err(msg) => return Ok(Self::err(&msg, 1)),
        };
        if !delete && !squeeze && non_flag_args.len() < 2 {
            return Ok(Self::err("missing operand after SET1", 1));
        }
        // GNU tr works on bytes: every SET element is a byte, and input is
        // translated byte by byte whatever its encoding.
        let set2 = match non_flag_args.get(1) {
            Some(spec) => match expand_char_set(spec) {
                Ok(set) => Some(set),
                Err(msg) => return Ok(Self::err(&msg, 1)),
            },
            // `-ds` with one SET squeezes that SET after deleting.
            None if delete && squeeze => Some(set1.clone()),
            None => None,
        };
        let stdin = ctx.stdin.cloned().unwrap_or_default();
        let output = translate_bytes(
            stdin.as_bytes(),
            &set1,
            set2.as_deref(),
            delete,
            squeeze,
            complement,
        );
        Ok(ExecResult::ok_bytes(output))
    }
}

fn translate_bytes(
    stdin: &[u8],
    set1: &[char],
    set2: Option<&[char]>,
    delete: bool,
    squeeze: bool,
    complement: bool,
) -> Vec<u8> {
    let original: Vec<u8> = set1.iter().map(|c| *c as u8).collect();
    let effective: Vec<u8> = if complement {
        (u8::MIN..=u8::MAX)
            .filter(|byte| !original.contains(byte))
            .collect()
    } else {
        original
    };
    let translated: Vec<u8> = set2
        .map(|set| set.iter().map(|c| *c as u8).collect())
        .unwrap_or_default();

    let mut output = Vec::with_capacity(stdin.len());
    for &byte in stdin {
        if delete && effective.contains(&byte) {
            continue;
        }
        let mapped = if !delete {
            effective
                .iter()
                .position(|candidate| *candidate == byte)
                .and_then(|position| translated.get(position).or(translated.last()))
                .copied()
                .unwrap_or(byte)
        } else {
            byte
        };
        let squeeze_set = if delete {
            &translated
        } else if translated.is_empty() {
            &effective
        } else {
            &translated
        };
        if squeeze && output.last() == Some(&mapped) && squeeze_set.contains(&mapped) {
            continue;
        }
        output.push(mapped);
    }
    output
}

/// One element of a tr SET operand: a character and whether it came from a
/// backslash escape (an escaped `-`, `[` or `:` is literal).
#[derive(Clone, Copy, PartialEq)]
struct SetAtom {
    ch: char,
    escaped: bool,
}

/// Split a SET operand into atoms, decoding GNU tr escapes: `\NNN` (one to
/// three octal digits, a byte), `\\`, `\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`.
/// Any other escaped character stands for itself.
fn set_atoms(spec: &str) -> Vec<SetAtom> {
    let chars: Vec<char> = spec.chars().collect();
    let mut atoms = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '\\' || i == chars.len() {
            // A literal non-ASCII character is its UTF-8 bytes.
            let mut buf = [0; 4];
            for &byte in c.encode_utf8(&mut buf).as_bytes() {
                atoms.push(SetAtom {
                    ch: char::from(byte),
                    escaped: false,
                });
            }
            continue;
        }
        let next = chars[i];
        i += 1;
        let ch = match next {
            '0'..='7' => {
                let mut value = next.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.get(i).and_then(|d| d.to_digit(8)) {
                        Some(digit) if value * 8 + digit <= 0o377 => {
                            value = value * 8 + digit;
                            i += 1;
                        }
                        _ => break,
                    }
                }
                char::from(value as u8)
            }
            'a' => '\x07',
            'b' => '\x08',
            'f' => '\x0c',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'v' => '\x0b',
            other => other,
        };
        atoms.push(SetAtom { ch, escaped: true });
    }
    atoms
}

/// Expand a character set specification like "a-z" into a list of characters.
/// Supports POSIX character classes: [:lower:], [:upper:], [:digit:], [:alpha:], [:alnum:], [:space:]
fn expand_char_set(spec: &str) -> std::result::Result<Vec<char>, String> {
    let mut chars = Vec::new();
    let atoms = set_atoms(spec);
    let len = atoms.len();
    let literal = |i: usize, ch: char| atoms.get(i) == Some(&SetAtom { ch, escaped: false });
    let mut i = 0;

    while i < len {
        // Check for POSIX character class [:class:]
        if literal(i, '[')
            && literal(i + 1, ':')
            && let Some(end) =
                (i + 2..len.saturating_sub(1)).find(|&j| literal(j, ':') && literal(j + 1, ']'))
        {
            let class_name: String = atoms[i + 2..end].iter().map(|atom| atom.ch).collect();
            match class_name.as_str() {
                "lower" => push_char_range(&mut chars, 'a', 'z')?,
                "upper" => push_char_range(&mut chars, 'A', 'Z')?,
                "digit" => push_char_range(&mut chars, '0', '9')?,
                "alpha" => {
                    push_char_range(&mut chars, 'a', 'z')?;
                    push_char_range(&mut chars, 'A', 'Z')?;
                }
                "alnum" => {
                    push_char_range(&mut chars, 'a', 'z')?;
                    push_char_range(&mut chars, 'A', 'Z')?;
                    push_char_range(&mut chars, '0', '9')?;
                }
                "space" => push_chars(&mut chars, [' ', '\t', '\n', '\r', '\x0b', '\x0c'])?,
                "blank" => push_chars(&mut chars, [' ', '\t'])?,
                "punct" => {
                    for code in 0x21u8..=0x7e {
                        let c = code as char;
                        if !c.is_ascii_alphanumeric() {
                            push_char(&mut chars, c)?;
                        }
                    }
                }
                "xdigit" => {
                    push_char_range(&mut chars, '0', '9')?;
                    push_char_range(&mut chars, 'A', 'F')?;
                    push_char_range(&mut chars, 'a', 'f')?;
                }
                "print" => {
                    for code in 0x20u8..=0x7e {
                        push_char(&mut chars, code as char)?;
                    }
                }
                "graph" => {
                    for code in 0x21u8..=0x7e {
                        push_char(&mut chars, code as char)?;
                    }
                }
                "cntrl" => {
                    for code in 0u8..=0x1f {
                        push_char(&mut chars, code as char)?;
                    }
                    push_char(&mut chars, 0x7f as char)?;
                }
                _ => {
                    push_char(&mut chars, '[')?;
                    i += 1;
                    continue;
                }
            }
            i = end + 2; // skip past [: + class + :]
            continue;
        }

        let c = atoms[i].ch;
        // Check for range like a-z (endpoints may be escapes, e.g. \000-\037)
        if i + 2 < len && literal(i + 1, '-') {
            push_char_range(&mut chars, c, atoms[i + 2].ch)?;
            i += 3;
        } else {
            push_char(&mut chars, c)?;
            i += 1;
        }
    }

    Ok(chars)
}

fn push_char(chars: &mut Vec<char>, ch: char) -> std::result::Result<(), String> {
    if chars.len() >= MAX_TR_EXPANDED_SET_CHARS {
        return Err("character set expansion too large".to_string());
    }
    chars.push(ch);
    Ok(())
}

fn push_chars(
    chars: &mut Vec<char>,
    values: impl IntoIterator<Item = char>,
) -> std::result::Result<(), String> {
    for ch in values {
        push_char(chars, ch)?;
    }
    Ok(())
}

fn push_char_range(
    chars: &mut Vec<char>,
    start: char,
    end: char,
) -> std::result::Result<(), String> {
    for code in (start as u32)..=(end as u32) {
        if let Some(ch) = char::from_u32(code) {
            push_char(chars, ch)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::fs::InMemoryFs;

    async fn run_cut(args: &[&str], stdin: Option<&str>) -> ExecResult {
        let fs = Arc::new(InMemoryFs::new());
        let mut variables = HashMap::new();
        let env = HashMap::new();
        let mut cwd = PathBuf::from("/");

        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let ctx = Context {
            args: &args,
            env: &env,
            variables: &mut variables,
            cwd: &mut cwd,
            fs,
            stdin: crate::builtins::test_stream_opt(stdin),
            #[cfg(feature = "http_client")]
            http_client: None,
            #[cfg(feature = "git")]
            git_client: None,
            #[cfg(feature = "ssh")]
            ssh_client: None,
            shell: None,
        };

        Cut.execute(ctx).await.unwrap()
    }

    async fn run_tr(args: &[&str], stdin: Option<&str>) -> ExecResult {
        let fs = Arc::new(InMemoryFs::new());
        let mut variables = HashMap::new();
        let env = HashMap::new();
        let mut cwd = PathBuf::from("/");

        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let ctx = Context {
            args: &args,
            env: &env,
            variables: &mut variables,
            cwd: &mut cwd,
            fs,
            stdin: crate::builtins::test_stream_opt(stdin),
            #[cfg(feature = "http_client")]
            http_client: None,
            #[cfg(feature = "git")]
            git_client: None,
            #[cfg(feature = "ssh")]
            ssh_client: None,
            shell: None,
        };

        Tr.execute(ctx).await.unwrap()
    }

    fn expanded(spec: &str) -> Vec<char> {
        expand_char_set(spec).unwrap()
    }

    #[tokio::test]
    async fn test_cut_single_field() {
        let result = run_cut(&["-d", ",", "-f", "2"], Some("a,b,c\n1,2,3\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "b\n2\n");
    }

    #[tokio::test]
    async fn test_cut_multiple_fields() {
        let result = run_cut(&["-d", ",", "-f", "1,3"], Some("a,b,c\n1,2,3\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "a,c\n1,3\n");
    }

    #[tokio::test]
    async fn test_cut_field_range() {
        let result = run_cut(&["-d", ",", "-f", "1-2"], Some("a,b,c,d\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "a,b\n");
    }

    #[tokio::test]
    async fn test_tr_lowercase_to_uppercase() {
        let result = run_tr(&["a-z", "A-Z"], Some("hello world")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "HELLO WORLD");
    }

    #[tokio::test]
    async fn test_tr_delete() {
        let result = run_tr(&["-d", "aeiou"], Some("hello world")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hll wrld");
    }

    #[tokio::test]
    async fn test_tr_single_char() {
        let result = run_tr(&[":", "-"], Some("a:b:c")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "a-b-c");
    }

    #[test]
    fn test_expand_char_set() {
        assert_eq!(expanded("abc"), vec!['a', 'b', 'c']);
        assert_eq!(expanded("a-c"), vec!['a', 'b', 'c']);
        assert_eq!(expanded("0-2"), vec!['0', '1', '2']);
    }

    #[test]
    fn test_expand_char_class_lower() {
        let lower = expanded("[:lower:]");
        assert_eq!(lower.len(), 26);
        assert_eq!(lower[0], 'a');
        assert_eq!(lower[25], 'z');
    }

    #[test]
    fn test_expand_char_class_upper() {
        let upper = expanded("[:upper:]");
        assert_eq!(upper.len(), 26);
        assert_eq!(upper[0], 'A');
        assert_eq!(upper[25], 'Z');
    }

    #[tokio::test]
    async fn test_tr_char_class_lower_to_upper() {
        let result = run_tr(&["[:lower:]", "[:upper:]"], Some("hello world\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "HELLO WORLD\n");
    }

    #[test]
    fn test_parse_position_spec() {
        // Resolved against 10 total positions
        let resolve = |spec: &str| resolve_positions(&parse_position_spec(spec), 10);
        assert_eq!(resolve("1"), vec![1]);
        assert_eq!(resolve("1,3"), vec![1, 3]);
        assert_eq!(resolve("1-3"), vec![1, 2, 3]);
        assert_eq!(resolve("1,3-5"), vec![1, 3, 4, 5]);
        assert_eq!(resolve("3-"), vec![3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(resolve("-3"), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_cut_char_mode() {
        let result = run_cut(&["-c", "1-5"], Some("hello world\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello\n");
    }

    #[tokio::test]
    async fn test_cut_complement() {
        let result = run_cut(&["-d", ",", "--complement", "-f", "2"], Some("a,b,c,d\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "a,c,d\n");
    }

    #[tokio::test]
    async fn test_cut_only_delimited() {
        let result = run_cut(
            &["-d", ",", "-f", "1", "-s"],
            Some("a,b,c\nno delim\nx,y\n"),
        )
        .await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "a\nx\n");
    }

    #[tokio::test]
    async fn test_tr_squeeze() {
        let result = run_tr(&["-s", "eol "], Some("heeelllo   wooorld\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "helo world\n");
    }

    #[tokio::test]
    async fn test_tr_complement_delete() {
        let result = run_tr(&["-cd", "0-9\n"], Some("hello123\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "123\n");
    }

    #[tokio::test]
    async fn test_tr_complement_uppercase_c() {
        // -C is POSIX alias for -c (complement)
        let result = run_tr(&["-Cd", "0-9\n"], Some("hello123\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "123\n");
    }

    #[tokio::test]
    async fn test_tr_combined_flags_ds() {
        // -ds: delete SET1 chars, then squeeze SET2 chars
        let result = run_tr(&["-ds", "aeiou", " "], Some("the  quick  fox\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "th qck fx\n");
    }

    #[test]
    fn test_expand_char_class_punct() {
        let punct = expanded("[:punct:]");
        assert!(punct.contains(&'!'));
        assert!(punct.contains(&'.'));
        assert!(punct.contains(&','));
        assert!(punct.contains(&'@'));
        assert!(punct.contains(&'#'));
        assert!(!punct.contains(&'a'));
        assert!(!punct.contains(&'0'));
        assert!(!punct.contains(&' '));
    }

    #[test]
    fn test_expand_char_class_xdigit() {
        let xdigit = expanded("[:xdigit:]");
        assert_eq!(xdigit.len(), 22); // 0-9 + A-F + a-f
        assert!(xdigit.contains(&'0'));
        assert!(xdigit.contains(&'9'));
        assert!(xdigit.contains(&'A'));
        assert!(xdigit.contains(&'F'));
        assert!(xdigit.contains(&'a'));
        assert!(xdigit.contains(&'f'));
        assert!(!xdigit.contains(&'G'));
        assert!(!xdigit.contains(&'g'));
    }

    #[test]
    fn test_expand_char_class_digit() {
        let digit = expanded("[:digit:]");
        assert_eq!(digit.len(), 10);
        assert_eq!(digit[0], '0');
        assert_eq!(digit[9], '9');
    }

    #[test]
    fn test_expand_char_class_alpha() {
        let alpha = expanded("[:alpha:]");
        assert_eq!(alpha.len(), 52);
        assert!(alpha.contains(&'a'));
        assert!(alpha.contains(&'z'));
        assert!(alpha.contains(&'A'));
        assert!(alpha.contains(&'Z'));
    }

    #[test]
    fn test_expand_char_class_alnum() {
        let alnum = expanded("[:alnum:]");
        assert_eq!(alnum.len(), 62);
        assert!(alnum.contains(&'a'));
        assert!(alnum.contains(&'0'));
        assert!(alnum.contains(&'Z'));
    }

    #[test]
    fn test_expand_char_class_space() {
        let space = expanded("[:space:]");
        assert!(space.contains(&' '));
        assert!(space.contains(&'\t'));
        assert!(space.contains(&'\n'));
        assert!(space.contains(&'\r'));
    }

    #[test]
    fn test_expand_char_class_blank() {
        let blank = expanded("[:blank:]");
        assert_eq!(blank.len(), 2);
        assert!(blank.contains(&' '));
        assert!(blank.contains(&'\t'));
    }

    #[test]
    fn test_expand_char_class_cntrl() {
        let cntrl = expanded("[:cntrl:]");
        assert!(cntrl.contains(&'\0'));
        assert!(cntrl.contains(&'\x1f'));
        assert!(cntrl.contains(&'\x7f'));
        assert!(!cntrl.contains(&' '));
    }

    #[tokio::test]
    async fn test_tr_delete_punct() {
        let result = run_tr(&["-d", "[:punct:]"], Some("hello, world!\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello world\n");
    }

    #[tokio::test]
    async fn test_tr_squeeze_spaces() {
        let result = run_tr(&["-s", "[:space:]"], Some("hello   world\n\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello world\n");
    }

    #[tokio::test]
    async fn test_tr_translate_with_squeeze() {
        let result = run_tr(&["-s", "a-z", "A-Z"], Some("aabbcc\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "ABC\n");
    }

    #[tokio::test]
    async fn test_cut_byte_mode() {
        // -b is alias for -c
        let result = run_cut(&["-b", "1-5"], Some("hello world\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello\n");
    }

    #[tokio::test]
    async fn test_cut_byte_mode_inline() {
        let result = run_cut(&["-b1,3,5"], Some("hello\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hlo\n");
    }

    #[tokio::test]
    async fn test_tr_complement_squeeze() {
        // -cs: complement SET1, then squeeze result chars in SET2
        let result = run_tr(&["-cs", "[:alpha:]", "\n"], Some("hello 123 world\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello\nworld\n");
    }

    #[tokio::test]
    async fn test_tr_multibyte_utf8() {
        // GNU tr works on bytes: ä is two bytes, each mapped to x.
        let result = run_tr(&["ä", "x"], Some("hällo\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hxxllo\n");
    }

    #[tokio::test]
    async fn test_tr_multibyte_utf8_range() {
        // SET1 "über" is five bytes; SET2's last byte pads the rest (GNU).
        let result = run_tr(&["über", "UBER"], Some("über\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "UBERR\n");
    }

    #[tokio::test]
    async fn test_cut_multibyte_utf8_chars() {
        // cut -c with multi-byte input selects chars not bytes
        let result = run_cut(&["-c", "1-3"], Some("äöü\n")).await;
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "äöü\n");
    }

    #[test]
    fn test_expand_char_set_rejects_large_expansion() {
        let err = expand_char_set(&"\\000-\\377".repeat(17)).unwrap_err();
        assert_eq!(err, "character set expansion too large");
    }

    #[tokio::test]
    async fn test_tr_rejects_large_expansion() {
        let result = run_tr(&[&"\\000-\\377".repeat(17), "x"], Some("abc")).await;
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.stderr, "tr: character set expansion too large\n");
    }

    #[test]
    fn test_expand_char_set_multibyte() {
        // Each UTF-8 byte of a literal is its own SET element.
        let chars = expanded("äö");
        assert_eq!(chars, vec!['\u{c3}', '\u{a4}', '\u{c3}', '\u{b6}']);
    }

    #[tokio::test]
    async fn test_cut_rejects_unknown_option() {
        let result = run_cut(&["-Q", "-f1"], Some("a\n")).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("invalid option -- 'Q'"));
    }

    #[tokio::test]
    async fn test_tr_rejects_unknown_option() {
        let result = run_tr(&["-Q", "a", "b"], Some("a\n")).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("invalid option -- 'Q'"));
    }
}
