use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::process::ExitCode;

use jsonl_peek::lines::LineReader;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("jsonl-peek: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> io::Result<ExitCode> {
    let mut args = env::args().skip(1);
    let command = match args.next() {
        Some(c) => c,
        None => {
            usage();
            return Ok(ExitCode::from(2));
        }
    };

    match command.as_str() {
        "head" => match parse_head_args(args) {
            Ok(parsed) => run_head(parsed),
            Err(msg) => {
                eprintln!("jsonl-peek: {msg}");
                usage();
                Ok(ExitCode::from(2))
            }
        },
        "-h" | "--help" => {
            usage();
            Ok(ExitCode::SUCCESS)
        }
        other => {
            eprintln!("jsonl-peek: unknown command '{other}'");
            usage();
            Ok(ExitCode::from(2))
        }
    }
}

struct HeadArgs {
    count: usize,
    file: Option<String>,
}

fn parse_head_args(mut args: impl Iterator<Item = String>) -> Result<HeadArgs, String> {
    let mut count = 10usize;
    let mut file = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" => {
                let value = args.next().ok_or_else(|| "-n requires a value".to_string())?;
                count = value
                    .parse()
                    .map_err(|_| format!("invalid count '{value}'"))?;
            }
            "-" => file = Some(arg),
            _ if arg.starts_with('-') => return Err(format!("unknown option '{arg}'")),
            _ if file.is_some() => return Err("too many file arguments".to_string()),
            _ => file = Some(arg),
        }
    }
    Ok(HeadArgs { count, file })
}

fn run_head(args: HeadArgs) -> io::Result<ExitCode> {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    match args.file.as_deref() {
        Some(path) if path != "-" => {
            let file = File::open(path)
                .map_err(|err| io::Error::new(err.kind(), format!("{path}: {err}")))?;
            head_from(BufReader::new(file), args.count, &mut out)?;
        }
        _ => {
            let stdin = io::stdin();
            head_from(stdin.lock(), args.count, &mut out)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn head_from<R: BufRead, W: Write>(reader: R, count: usize, out: &mut W) -> io::Result<()> {
    let mut lines = LineReader::new(reader);
    let mut shown = 0;
    while shown < count {
        match lines.next_line()? {
            Some(line) => {
                out.write_all(line.bytes)?;
                out.write_all(b"\n")?;
                shown += 1;
            }
            None => break,
        }
    }
    Ok(())
}

fn usage() {
    eprintln!("usage: jsonl-peek head [-n N] [FILE]");
}
