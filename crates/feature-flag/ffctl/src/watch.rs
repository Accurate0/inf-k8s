use feature_flag_proto as pb;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDelta {
    pub version: i64,
    pub initial: bool,
    pub flags: usize,
    pub segments: usize,
    pub changed: Vec<String>,
}

impl SnapshotDelta {
    pub fn summary(&self) -> String {
        if self.initial {
            return format!(
                "version {}: connected, {} flag(s), {} segment(s)",
                self.version, self.flags, self.segments
            );
        }

        if self.changed.is_empty() {
            return format!("version {}: no visible changes", self.version);
        }

        format!("version {}: {}", self.version, self.changed.join(", "))
    }
}

#[derive(Default)]
pub struct SnapshotTracker {
    seen: bool,
    flags: BTreeMap<String, pb::Flag>,
    segments: BTreeMap<String, pb::Segment>,
}

impl SnapshotTracker {
    pub fn observe(&mut self, snapshot: pb::SnapshotResponse) -> SnapshotDelta {
        let flags: BTreeMap<String, pb::Flag> = snapshot
            .flags
            .into_iter()
            .map(|flag| (flag.key.clone(), flag))
            .collect();

        let segments: BTreeMap<String, pb::Segment> = snapshot
            .segments
            .into_iter()
            .map(|segment| (segment.key.clone(), segment))
            .collect();

        let mut changed = Self::changed("flag", &self.flags, &flags);
        changed.extend(Self::changed("segment", &self.segments, &segments));

        let delta = SnapshotDelta {
            version: snapshot.version,
            initial: !self.seen,
            flags: flags.len(),
            segments: segments.len(),
            changed: if self.seen { changed } else { Vec::new() },
        };

        self.seen = true;
        self.flags = flags;
        self.segments = segments;

        delta
    }

    fn changed<T: PartialEq>(
        kind: &str,
        before: &BTreeMap<String, T>,
        after: &BTreeMap<String, T>,
    ) -> Vec<String> {
        let mut changed = Vec::new();

        for (key, value) in after {
            match before.get(key) {
                None => changed.push(format!("+{kind} {key}")),
                Some(previous) if previous != value => changed.push(format!("~{kind} {key}")),
                Some(_) => {}
            }
        }

        for key in before.keys() {
            if !after.contains_key(key) {
                changed.push(format!("-{kind} {key}"));
            }
        }

        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(key: &str, enabled: bool) -> pb::Flag {
        pb::Flag {
            key: key.to_owned(),
            enabled,
            ..Default::default()
        }
    }

    fn snapshot(version: i64, flags: Vec<pb::Flag>) -> pb::SnapshotResponse {
        pb::SnapshotResponse {
            version,
            flags,
            segments: Vec::new(),
        }
    }

    #[test]
    fn first_snapshot_is_initial_and_reports_no_changes() {
        let mut tracker = SnapshotTracker::default();
        let delta = tracker.observe(snapshot(1, vec![flag("a", true)]));

        assert!(delta.initial);
        assert!(delta.changed.is_empty());
        assert_eq!(delta.flags, 1);
    }

    #[test]
    fn later_snapshots_report_added_changed_and_removed() {
        let mut tracker = SnapshotTracker::default();
        tracker.observe(snapshot(1, vec![flag("a", true), flag("b", true)]));

        let delta = tracker.observe(snapshot(2, vec![flag("a", false), flag("c", true)]));

        assert!(!delta.initial);
        assert_eq!(delta.changed, vec!["~flag a", "+flag c", "-flag b"]);
    }
}
