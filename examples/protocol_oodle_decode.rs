//! Reference-only bounded Oodle research helper, not a native-server dependency.
//! The caller selects the authorized members and enforces process resource limits.
//! Usage: protocol_oodle_decode COMPRESSED_LEN DECODED_LEN < member > decoded

use std::io::{self, Cursor, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

const MAX_LEN: usize = 64 * 1024 * 1024;

fn length(value: &str) -> Result<usize, ()> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    let value = value.parse::<usize>().map_err(|_| ())?;
    if value == 0 || value > MAX_LEN {
        return Err(());
    }
    Ok(value)
}

fn lengths(args: impl IntoIterator<Item = String>) -> Result<(usize, usize), ()> {
    let mut args = args.into_iter();
    let compressed = length(&args.next().ok_or(())?)?;
    let decoded = length(&args.next().ok_or(())?)?;
    if args.next().is_some() {
        return Err(());
    }
    Ok((compressed, decoded))
}

fn decode(mut input: impl Read, compressed: usize, decoded: usize) -> Result<Vec<u8>, ()> {
    // Also validate here so callers cannot bypass the pre-allocation bounds.
    if compressed == 0 || decoded == 0 || compressed > MAX_LEN || decoded > MAX_LEN {
        return Err(());
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(compressed + 1).map_err(|_| ())?;
    input
        .by_ref()
        .take((compressed + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() != compressed {
        return Err(());
    }
    catch_unwind(AssertUnwindSafe(|| {
        let mut output = Vec::new();
        output.try_reserve_exact(decoded).map_err(|_| ())?;
        output.resize(decoded, 0);
        let mut cursor = Cursor::new(bytes.as_slice());
        let written = oozextract::Extractor::new()
            .read(&mut cursor, &mut output)
            .map_err(|_| ())?;
        if written != decoded || cursor.position() != compressed as u64 {
            return Err(());
        }
        Ok(output)
    }))
    .map_err(|_| ())?
}

fn run() -> Result<(), ()> {
    let (compressed, decoded) = lengths(std::env::args().skip(1))?;
    let output = decode(io::stdin().lock(), compressed, decoded)?;
    io::stdout().lock().write_all(&output).map_err(|_| ())
}

fn main() -> std::process::ExitCode {
    // CLI-local suppression: malformed streams must not print decoder internals.
    std::panic::set_hook(Box::new(|_| {}));
    match catch_unwind(run).unwrap_or(Err(())) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => {
            eprintln!("reference decode failed");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Authored uncompressed Kraken block: 0x4c means uncompressed, 0x06 Kraken.
    const FIXTURE: &[u8] = b"\x4c\x06hello";

    #[test]
    fn authored_uncompressed_block() {
        assert_eq!(decode(FIXTURE, FIXTURE.len(), 5), Ok(b"hello".to_vec()));
    }

    #[test]
    fn rejects_truncation_suffix_and_wrong_output_length() {
        assert!(decode(&FIXTURE[..6], FIXTURE.len(), 5).is_err());
        assert!(decode(&FIXTURE[..6], 6, 5).is_err());
        assert!(decode(&b"\x4c\x06hello!"[..], 7, 5).is_err());
        assert!(decode(&b"\x4c\x06hello!"[..], 8, 5).is_err());
        assert!(decode(FIXTURE, FIXTURE.len(), 4).is_err());
        assert!(decode(FIXTURE, FIXTURE.len(), 6).is_err());
    }

    #[test]
    fn validates_decimal_arguments_and_limits() {
        for invalid in [
            "",
            "0",
            "-1",
            "+1",
            " 1",
            "1.0",
            "0x10",
            "67108865",
            "184467440737095516160",
        ] {
            assert!(length(invalid).is_err());
        }
        assert_eq!(length("67108864"), Ok(MAX_LEN));
        assert_eq!(length("0007"), Ok(7));
        for args in [vec![], vec!["7"], vec!["7", "5", "1"]] {
            assert!(lengths(args.into_iter().map(str::to_owned)).is_err());
        }
        assert_eq!(lengths(["7".to_owned(), "5".to_owned()]), Ok((7, 5)));
    }

    #[test]
    fn invalid_limits_do_not_read_input() {
        struct MustNotRead;
        impl Read for MustNotRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("invalid lengths must be rejected before reading");
            }
        }
        for (compressed, decoded) in [(0, 5), (7, 0), (MAX_LEN + 1, 5), (7, MAX_LEN + 1)] {
            assert!(decode(MustNotRead, compressed, decoded).is_err());
        }
    }
}
