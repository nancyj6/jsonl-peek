use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonl_peek::hist::Histogram;
use jsonl_peek::json::Json;
use jsonl_peek::lines::LineReader;
use jsonl_peek::path::FieldPath;
use jsonl_peek::rng::{Reservoir, SplitMix64};
use jsonl_peek::stats::{FieldStats, Issue, Stats, StatsOptions};

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
        "stats" => match parse_stats_args(args) {
            Ok(parsed) => run_stats(parsed),
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

struct StatsArgs {
    fields: Vec<FieldPath>,
    top: usize,
    max_errors: usize,
    json: bool,
    file: Option<String>,
}

fn parse_stats_args(mut args: impl Iterator<Item = String>) -> Result<StatsArgs, String> {
    let mut fields = Vec::new();
    let mut top = 10usize;
    let mut max_errors = 10usize;
    let mut json = false;
    let mut file = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--field" => {
                let value = args.next().ok_or_else(|| "--field requires a value".to_string())?;
                let path = FieldPath::parse(&value).map_err(|err| format!("--field '{value}': {err}"))?;
                fields.push(path);
            }
            "--top" => {
                let value = args.next().ok_or_else(|| "--top requires a value".to_string())?;
                top = value.parse().map_err(|_| format!("invalid count '{value}'"))?;
            }
            "--max-errors" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--max-errors requires a value".to_string())?;
                max_errors = value.parse().map_err(|_| format!("invalid count '{value}'"))?;
            }
            "--json" => json = true,
            "-" => file = Some(arg),
            _ if arg.starts_with('-') => return Err(format!("unknown option '{arg}'")),
            _ if file.is_some() => return Err("too many file arguments".to_string()),
            _ => file = Some(arg),
        }
    }
    Ok(StatsArgs { fields, top, max_errors, json, file })
}

fn run_stats(args: StatsArgs) -> io::Result<ExitCode> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let top = args.top;
    let json = args.json;
    let options = StatsOptions { fields: args.fields, top: args.top, max_errors: args.max_errors };

    match args.file.as_deref() {
        Some(path) if path != "-" => {
            let file = File::open(path)
                .map_err(|err| io::Error::new(err.kind(), format!("{path}: {err}")))?;
            let bytes = file.metadata().ok().map(|meta| meta.len());
            let stats = Stats::from_reader(BufReader::new(file), options)?;
            if json {
                print_stats_json(&stats, top, path, bytes, &mut out)?;
            } else {
                print_stats(&stats, top, path, bytes, &mut out)?;
            }
        }
        _ => {
            let stdin = io::stdin();
            let stats = Stats::from_reader(stdin.lock(), options)?;
            if json {
                print_stats_json(&stats, top, "-", None, &mut out)?;
            } else {
                print_stats(&stats, top, "-", None, &mut out)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_stats_json<W: Write>(
    stats: &Stats,
    top: usize,
    file_label: &str,
    bytes: Option<u64>,
    out: &mut W,
) -> io::Result<()> {
    stats_to_json(stats, top, file_label, bytes).write(out)?;
    writeln!(out)
}

fn stats_to_json(stats: &Stats, top: usize, file_label: &str, bytes: Option<u64>) -> Json {
    let invalid = stats.lines - stats.blank - stats.valid;
    let mut members = vec![
        ("file", Json::Str(file_label.to_string())),
        ("lines", Json::UInt(stats.lines as u64)),
        ("blank", Json::UInt(stats.blank as u64)),
        ("invalid", Json::UInt(invalid as u64)),
        ("valid", Json::UInt(stats.valid as u64)),
    ];
    if let Some(bytes) = bytes {
        members.push(("bytes", Json::UInt(bytes)));
    }
    members.push(("top_level_types", type_counts_json(&stats.top_level_types.most_common())));
    members.push(("line_length", line_length_json(&stats.line_length)));
    members.push(("fields", Json::Array(stats.fields.iter().map(|field| field_json(field, top)).collect())));
    members.push(("issues", Json::Array(stats.issues.iter().map(issue_json).collect())));
    members.push(("issues_truncated", Json::Bool(stats.issues_truncated)));
    Json::Object(members)
}

fn type_counts_json(counts: &[(&'static str, u64)]) -> Json {
    Json::Object(counts.iter().map(|&(name, count)| (name, Json::UInt(count))).collect())
}

fn line_length_json(hist: &Histogram) -> Json {
    Json::Object(vec![
        ("count", Json::UInt(hist.count())),
        ("min", Json::UInt(hist.min().unwrap_or(0))),
        ("p50", Json::UInt(hist.percentile(0.5).unwrap_or(0))),
        ("p90", Json::UInt(hist.percentile(0.9).unwrap_or(0))),
        ("p99", Json::UInt(hist.percentile(0.99).unwrap_or(0))),
        ("max", Json::UInt(hist.max().unwrap_or(0))),
        ("mean", Json::Float(hist.mean().unwrap_or(0.0))),
    ])
}

fn field_json(field: &FieldStats, top: usize) -> Json {
    Json::Object(vec![
        ("path", Json::Str(field.path.to_string())),
        ("records_present", Json::UInt(field.records_present as u64)),
        ("value_count", Json::UInt(field.value_count)),
        ("types", type_counts_json(&field.types.most_common())),
        ("distinct", Json::UInt(field.distinct() as u64)),
        ("values_truncated", Json::Bool(field.values_truncated)),
        (
            "top",
            Json::Array(
                field
                    .top(top)
                    .into_iter()
                    .map(|(value, count)| Json::Object(vec![("value", Json::Str(value.to_string())), ("count", Json::UInt(count))]))
                    .collect(),
            ),
        ),
    ])
}

fn issue_json(issue: &Issue) -> Json {
    Json::Object(vec![
        ("line", Json::UInt(issue.line as u64)),
        ("column", Json::UInt(issue.column as u64)),
        ("reason", Json::Str(issue.reason.clone())),
    ])
}

fn print_stats<W: Write>(
    stats: &Stats,
    top: usize,
    file_label: &str,
    bytes: Option<u64>,
    out: &mut W,
) -> io::Result<()> {
    let invalid = stats.lines - stats.blank - stats.valid;

    writeln!(out, "file    {file_label}")?;
    writeln!(
        out,
        "lines  {:>10}   blank {}   invalid {}   valid {}",
        format_count(stats.lines as u64),
        stats.blank,
        invalid,
        format_count(stats.valid as u64),
    )?;
    if let Some(bytes) = bytes {
        writeln!(out, "bytes  {:>10}   ({})", format_count(bytes), format_bytes(bytes))?;
    }
    writeln!(out, "top level  {}", join_type_counts(&stats.top_level_types.most_common()))?;

    if stats.line_length.count() > 0 {
        writeln!(out)?;
        writeln!(out, "line length in bytes")?;
        writeln!(
            out,
            "  min {}   p50 {}   p90 {}   p99 {}   max {}   mean {:.1}",
            stats.line_length.min().unwrap_or(0),
            stats.line_length.percentile(0.5).unwrap_or(0),
            stats.line_length.percentile(0.9).unwrap_or(0),
            stats.line_length.percentile(0.99).unwrap_or(0),
            stats.line_length.max().unwrap_or(0),
            stats.line_length.mean().unwrap_or(0.0),
        )?;
    }

    for field in &stats.fields {
        writeln!(out)?;
        writeln!(out, "field {}", field.path)?;
        writeln!(
            out,
            "  present in {} of {} records ({}), {} values, types {}",
            format_count(field.records_present as u64),
            format_count(stats.valid as u64),
            percent(field.records_present, stats.valid),
            format_count(field.value_count),
            join_type_counts(&field.types.most_common()),
        )?;
        let distinct_suffix = if field.values_truncated { " (truncated)" } else { "" };
        writeln!(out, "  {} distinct values{distinct_suffix}", format_count(field.distinct() as u64))?;
        for (value, count) in field.top(top) {
            writeln!(
                out,
                "  {:>10}  {:>6}  {value}",
                format_count(count),
                percent(count as usize, field.value_count as usize),
            )?;
        }
    }

    if !stats.issues.is_empty() || stats.issues_truncated {
        writeln!(out)?;
        writeln!(out, "invalid lines ({} total, showing {})", format_count(invalid as u64), stats.issues.len())?;
        for issue in &stats.issues {
            writeln!(out, "  line {} col {}: {}", issue.line, issue.column, issue.reason)?;
        }
    }
    Ok(())
}

fn join_type_counts(counts: &[(&str, u64)]) -> String {
    counts
        .iter()
        .map(|(name, count)| format!("{name}:{}", format_count(*count)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders `part` as a percentage of `total` with one decimal place, e.g.
/// `25.1%`. `0.0%` when `total` is zero rather than dividing by it.
fn percent(part: usize, total: usize) -> String {
    if total == 0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", part as f64 * 100.0 / total as f64)
    }
}

/// Renders `n` with `,` as a thousands separator, e.g. `2,003`.
fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Renders a byte count in binary units, e.g. `914.8 KiB`.
fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn usage() {
    eprintln!("usage: jsonl-peek head   [-n N] [FILE]");
    eprintln!("       jsonl-peek sample [-n N] [--seed S] [FILE]");
    eprintln!("       jsonl-peek stats  [--field PATH]... [--top N] [--max-errors N] [--json] [FILE]");
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn format_count_groups_by_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(5), "5");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(2_003), "2,003");
        assert_eq!(format_count(1_000_000), "1,000,000");
    }

    #[test]
    fn percent_formats_one_decimal_and_avoids_division_by_zero() {
        assert_eq!(percent(1, 4), "25.0%");
        assert_eq!(percent(2, 3), "66.7%");
        assert_eq!(percent(0, 0), "0.0%");
    }

    #[test]
    fn format_bytes_picks_the_largest_fitting_unit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(936_782), "914.8 KiB");
        assert_eq!(format_bytes(1_048_576), "1.0 MiB");
    }
}
