use super::*;

#[derive(Default)]
struct Record(Vec<String>);

impl Progress for Record {
    fn begin(&mut self, item: &str) {
        self.0.push(format!("begin {item}"));
    }
    fn phase(&mut self, phase: Phase, total: Option<u64>) {
        self.0.push(format!("{} {total:?}", phase.as_str()));
    }
    fn advance(&mut self, n: u64) {
        self.0.push(format!("+{n}"));
    }
    fn end(&mut self, ok: bool) {
        self.0.push(format!("end {ok}"));
    }
}

#[test]
fn reader_reports_bytes() {
    let mut record = Record::default();
    let cancel = CancelFlag::new();
    let mut out = Vec::new();
    io::copy(
        &mut ProgressReader::new(io::Cursor::new(vec![1u8; 10]), &mut record, &cancel),
        &mut out,
    )
    .unwrap();
    assert_eq!(out.len(), 10);
    let total: u64 = record
        .0
        .iter()
        .filter_map(|e| e.strip_prefix('+')?.parse::<u64>().ok())
        .sum();
    assert_eq!(total, 10);
}

#[test]
fn cancel_stops_a_copy_from_another_handle() {
    let mut record = Record::default();
    let cancel = CancelFlag::new();
    let other = cancel.clone();
    assert!(cancel.check().is_ok());
    other.cancel();
    assert_eq!(cancel.check(), Err(Cancelled));
    let err = io::copy(
        &mut ProgressReader::new(io::Cursor::new(vec![0u8; 10]), &mut record, &cancel),
        &mut io::sink(),
    )
    .unwrap_err();
    assert!(is_cancelled(&err), "{err}");
    assert!(!is_cancelled(&io::Error::other("x")));
    assert!(record.0.is_empty());
}

#[test]
fn no_progress_accepts_everything() {
    let mut p = NoProgress;
    p.begin("x");
    p.phase(Phase::Verify, Some(1));
    p.advance(1);
    p.end(true);
    assert_eq!(Phase::Commit.as_str(), "installing");
}
