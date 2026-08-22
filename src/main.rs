use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonl_peek::lines::LineReader;
use jsonl_peek::rng::{Reservoir, SplitMix64};

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
        "sample" => match parse_sample_args(args) {
            Ok(parsed) => run_sample(parsed),
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

struct SampleArgs {
    count: usize,
    seed: Option<u64>,
    file: Option<String>,
}

fn parse_sample_args(mut args: impl Iterator<Item = String>) -> Result<SampleArgs, String> {
    let mut count = 10usize;
    let mut seed = None;
    let mut file = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" => {
                let value = args.next().ok_or_else(|| "-n requires a value".to_string())?;
                count = value
                    .parse()
                    .map_err(|_| format!("invalid count '{value}'"))?;
            }
            "--seed" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--seed requires a value".to_string())?;
                seed = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid seed '{value}'"))?,
                );
            }
            "-" => file = Some(arg),
            _ if arg.starts_with('-') => return Err(format!("unknown option '{arg}'")),
            _ if file.is_some() => return Err("too many file arguments".to_string()),
            _ => file = Some(arg),
        }
    }
    Ok(SampleArgs { count, seed, file })
}

fn run_sample(args: SampleArgs) -> io::Result<ExitCode> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let seed = args.seed.unwrap_or_else(random_seed);

    match args.file.as_deref() {
        Some(path) if path != "-" => {
            let file = File::open(path)
                .map_err(|err| io::Error::new(err.kind(), format!("{path}: {err}")))?;
            sample_from(BufReader::new(file), args.count, seed, &mut out)?;
        }
        _ => {
            let stdin = io::stdin();
            sample_from(stdin.lock(), args.count, seed, &mut out)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A seed derived from the clock, used when `--seed` is not given. Sampling
/// is still uniform - this only decides which run of the program gets which
/// draw from the space of possible reservoirs.
fn random_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

fn sample_from<R: BufRead, W: Write>(
    reader: R,
    count: usize,
    seed: u64,
    out: &mut W,
) -> io::Result<()> {
    let mut lines = LineReader::new(reader);
    let mut rng = SplitMix64::new(seed);
    let mut reservoir = Reservoir::new(count);
    while let Some(line) = lines.next_line()? {
        if line.bytes.is_empty() {
            continue;
        }
        reservoir.add((line.number, line.bytes.to_vec()), &mut rng);
    }

    // The reservoir does not preserve arrival order; sort the selection back
    // into original file order before printing it.
    let mut items = reservoir.into_items();
    items.sort_unstable_by_key(|(number, _)| *number);
    for (_, bytes) in items {
        out.write_all(&bytes)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn usage() {
    eprintln!("usage: jsonl-peek head   [-n N] [FILE]");
    eprintln!("       jsonl-peek sample [-n N] [--seed S] [FILE]");
}
