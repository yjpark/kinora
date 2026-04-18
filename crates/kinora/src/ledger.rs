use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::event::{Event, EventError};
use crate::hash::{Hash, SHORTHASH_LEN};
use crate::paths::{head_path, ledger_dir, ledger_file_path};

#[derive(Debug)]
pub enum LedgerError {
    Io(io::Error),
    Event(EventError),
    NoHead,
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LedgerError::Io(e) => write!(f, "ledger io error: {e}"),
            LedgerError::Event(e) => write!(f, "ledger event error: {e}"),
            LedgerError::NoHead => write!(f, "no HEAD: call mint_and_append for the first event"),
        }
    }
}

impl std::error::Error for LedgerError {}

impl From<io::Error> for LedgerError {
    fn from(e: io::Error) -> Self {
        LedgerError::Io(e)
    }
}

impl From<EventError> for LedgerError {
    fn from(e: EventError) -> Self {
        LedgerError::Event(e)
    }
}

pub struct Ledger {
    kinora_root: PathBuf,
}

impl Ledger {
    pub fn new(kinora_root: impl Into<PathBuf>) -> Self {
        Self { kinora_root: kinora_root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.kinora_root
    }

    pub fn ensure_layout(&self) -> Result<(), LedgerError> {
        fs::create_dir_all(ledger_dir(&self.kinora_root))?;
        Ok(())
    }

    /// Read the current lineage shorthash from `.kinora/HEAD`, if any.
    pub fn current_lineage(&self) -> Result<Option<String>, LedgerError> {
        let path = head_path(&self.kinora_root);
        match fs::read_to_string(&path) {
            Ok(s) => Ok(Some(s.trim().to_owned())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(LedgerError::Io(e)),
        }
    }

    /// Write `.kinora/HEAD` with the given lineage shorthash.
    pub fn set_head(&self, shorthash: &str) -> Result<(), LedgerError> {
        let path = head_path(&self.kinora_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, shorthash)?;
        Ok(())
    }

    /// Mint a new lineage file from `event`, set HEAD to its shorthash, and
    /// return the shorthash.
    pub fn mint_and_append(&self, event: &Event) -> Result<String, LedgerError> {
        self.ensure_layout()?;
        let line = event.to_json_line()?;
        let shorthash =
            Hash::of_content(line.as_bytes()).shorthash().to_owned();
        let file = ledger_file_path(&self.kinora_root, &shorthash);
        write_line(&file, &line)?;
        self.set_head(&shorthash)?;
        Ok(shorthash)
    }

    /// Append `event` to the current HEAD lineage file. Returns the shorthash
    /// on success, or `LedgerError::NoHead` if HEAD is not set.
    pub fn append_to_head(&self, event: &Event) -> Result<String, LedgerError> {
        let shorthash = self.current_lineage()?.ok_or(LedgerError::NoHead)?;
        let file = ledger_file_path(&self.kinora_root, &shorthash);
        let line = event.to_json_line()?;
        append_line(&file, &line)?;
        Ok(shorthash)
    }

    /// Read all events from a single lineage file by shorthash.
    pub fn read_lineage(&self, shorthash: &str) -> Result<Vec<Event>, LedgerError> {
        let file = ledger_file_path(&self.kinora_root, shorthash);
        read_events(&file)
    }

    /// Read all lineage files under `.kinora/ledger/` keyed by shorthash.
    pub fn read_all_lineages(&self) -> Result<BTreeMap<String, Vec<Event>>, LedgerError> {
        let dir = ledger_dir(&self.kinora_root);
        if !dir.exists() {
            return Ok(BTreeMap::new());
        }
        let mut out = BTreeMap::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if s.len() == SHORTHASH_LEN => s.to_owned(),
                _ => continue,
            };
            let events = read_events(&path)?;
            out.insert(stem, events);
        }
        Ok(out)
    }
}

fn write_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

fn append_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

fn read_events(path: &Path) -> Result<Vec<Event>, LedgerError> {
    let f = fs::File::open(path)?;
    let r = BufReader::new(f);
    let mut out = Vec::new();
    for line in r.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(Event::from_json_line(&line)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn ledger() -> (TempDir, Ledger) {
        let tmp = TempDir::new().unwrap();
        let kin = tmp.path().to_path_buf();
        let l = Ledger::new(&kin);
        l.ensure_layout().unwrap();
        (tmp, l)
    }

    fn event(n: &str, ts: &str) -> Event {
        let h = Hash::of_content(n.as_bytes());
        Event {
            kind: "markdown".into(),
            id: h.as_hex().into(),
            hash: h.as_hex().into(),
            parents: vec![],
            ts: ts.into(),
            author: "yj".into(),
            provenance: "test".into(),
            metadata: BTreeMap::from([("name".to_string(), n.to_string())]),
        }
    }

    #[test]
    fn head_starts_absent() {
        let (_t, l) = ledger();
        assert!(l.current_lineage().unwrap().is_none());
    }

    #[test]
    fn mint_creates_lineage_and_sets_head() {
        let (_t, l) = ledger();
        let e = event("first", "2026-04-18T09:00:00Z");
        let sh = l.mint_and_append(&e).unwrap();
        assert_eq!(sh.len(), SHORTHASH_LEN);
        assert_eq!(l.current_lineage().unwrap(), Some(sh.clone()));
        let read = l.read_lineage(&sh).unwrap();
        assert_eq!(read, vec![e]);
    }

    #[test]
    fn append_to_head_requires_existing_head() {
        let (_t, l) = ledger();
        let e = event("x", "2026-04-18T09:00:00Z");
        let err = l.append_to_head(&e).unwrap_err();
        assert!(matches!(err, LedgerError::NoHead));
    }

    #[test]
    fn append_adds_events_preserving_order() {
        let (_t, l) = ledger();
        let a = event("a", "2026-04-18T09:00:00Z");
        let b = event("b", "2026-04-18T09:01:00Z");
        let c = event("c", "2026-04-18T09:02:00Z");
        let sh = l.mint_and_append(&a).unwrap();
        l.append_to_head(&b).unwrap();
        l.append_to_head(&c).unwrap();
        let events = l.read_lineage(&sh).unwrap();
        assert_eq!(events, vec![a, b, c]);
    }

    #[test]
    fn ledger_is_append_only_prior_entries_not_modified() {
        let (_t, l) = ledger();
        let a = event("a", "2026-04-18T09:00:00Z");
        let sh = l.mint_and_append(&a).unwrap();
        let file = ledger_file_path(l.root(), &sh);
        let before = fs::read(&file).unwrap();
        let b = event("b", "2026-04-18T09:01:00Z");
        l.append_to_head(&b).unwrap();
        let after = fs::read(&file).unwrap();
        assert!(after.starts_with(&before), "prior content mutated");
    }

    #[test]
    fn read_all_lineages_collects_files() {
        let (_t, l) = ledger();
        let a = event("a", "2026-04-18T09:00:00Z");
        let sh_a = l.mint_and_append(&a).unwrap();

        let b = event("b", "2026-04-18T09:00:01Z");
        let sh_b = Hash::of_content(b.to_json_line().unwrap().as_bytes())
            .shorthash()
            .to_owned();
        let other = ledger_file_path(l.root(), &sh_b);
        write_line(&other, &b.to_json_line().unwrap()).unwrap();

        let all = l.read_all_lineages().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains_key(&sh_a));
        assert!(all.contains_key(&sh_b));
    }

    #[test]
    fn read_all_lineages_skips_non_jsonl() {
        let (_t, l) = ledger();
        let a = event("a", "2026-04-18T09:00:00Z");
        l.mint_and_append(&a).unwrap();
        fs::write(ledger_dir(l.root()).join("notes.txt"), b"scratch").unwrap();
        let all = l.read_all_lineages().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn lineage_filename_matches_shorthash_of_first_event() {
        let (_t, l) = ledger();
        let a = event("a", "2026-04-18T09:00:00Z");
        let line = a.to_json_line().unwrap();
        let expected = Hash::of_content(line.as_bytes()).shorthash().to_owned();
        let sh = l.mint_and_append(&a).unwrap();
        assert_eq!(sh, expected);
    }
}
