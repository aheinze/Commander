//! Bounded line alignment with a linear fallback for very large differences.
use dualpane_core::{CancelToken, EntryKind, VPath};
use dualpane_vfs::Vfs;
use std::io::Read;
const MAX_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct DiffRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub changed: bool,
}
#[derive(Clone, Debug)]
pub struct FileComparison {
    pub left: Vec<String>,
    pub right: Vec<String>,
    pub rows: Vec<DiffRow>,
    pub binary: bool,
}
pub fn load(
    vfs: &dyn Vfs,
    left: &VPath,
    right: &VPath,
    ignore_case: bool,
    ignore_space: bool,
    cancel: &CancelToken,
) -> Result<FileComparison, String> {
    let read = |path: &VPath| -> Result<Vec<u8>, String> {
        cancel
            .check()
            .map_err(|_| "Comparison cancelled".to_owned())?;
        let before = vfs.stat(path, false).map_err(|e| e.to_string())?;
        if before.kind != EntryKind::File || before.size > MAX_BYTES {
            return Err(format!("Choose regular files up to 8 MiB: {path}"));
        }
        let mut data = Vec::new();
        vfs.open_read(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        if data.len() as u64 > MAX_BYTES {
            return Err(format!("File exceeds 8 MiB: {path}"));
        }
        let after = vfs.stat(path, false).map_err(|e| e.to_string())?;
        if before.identity != after.identity
            || before.size != after.size
            || before.modified != after.modified
        {
            return Err(format!(
                "File changed while reading: {path}. Compare again."
            ));
        }
        Ok(data)
    };
    let a = read(left)?;
    let b = read(right)?;
    let binary = a.contains(&0)
        || b.contains(&0)
        || std::str::from_utf8(&a).is_err()
        || std::str::from_utf8(&b).is_err();
    let lines = |bytes: &[u8]| -> Vec<String> {
        if binary {
            bytes
                .chunks(16)
                .map(|chunk| {
                    chunk
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        } else {
            std::str::from_utf8(bytes)
                .unwrap()
                .split_inclusive('\n')
                .map(str::to_owned)
                .collect()
        }
    };
    let left = lines(&a);
    let right = lines(&b);
    if left.len().max(right.len()) > 100_000 {
        return Err(
            "Comparison exceeds 100,000 rows. Use an external comparison tool for this file."
                .into(),
        );
    }
    let rows = align(
        &left,
        &right,
        !binary && ignore_case,
        !binary && ignore_space,
        cancel,
    )?;
    Ok(FileComparison {
        left,
        right,
        rows,
        binary,
    })
}
pub fn align(
    left: &[String],
    right: &[String],
    ignore_case: bool,
    ignore_space: bool,
    cancel: &CancelToken,
) -> Result<Vec<DiffRow>, String> {
    let normalize = |s: &String| {
        let s = if ignore_space {
            s.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            s.clone()
        };
        if ignore_case { s.to_lowercase() } else { s }
    };
    let a: Vec<_> = left.iter().map(normalize).collect();
    let b: Vec<_> = right.iter().map(normalize).collect();
    let mut prefix = 0;
    while prefix < a.len().min(b.len()) && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut end_a = a.len();
    let mut end_b = b.len();
    while end_a > prefix && end_b > prefix && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let mut rows: Vec<_> = (0..prefix)
        .map(|i| DiffRow {
            left: Some(i),
            right: Some(i),
            changed: false,
        })
        .collect();
    let n = end_a - prefix;
    let m = end_b - prefix;
    if (n + 1).saturating_mul(m + 1) <= 4_000_000 {
        let mut lengths = vec![0_u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            cancel
                .check()
                .map_err(|_| "Comparison cancelled".to_owned())?;
            for j in (0..m).rev() {
                lengths[i * (m + 1) + j] = if a[prefix + i] == b[prefix + j] {
                    1 + lengths[(i + 1) * (m + 1) + j + 1]
                } else {
                    lengths[(i + 1) * (m + 1) + j].max(lengths[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && a[prefix + i] == b[prefix + j] {
                rows.push(DiffRow {
                    left: Some(prefix + i),
                    right: Some(prefix + j),
                    changed: false,
                });
                i += 1;
                j += 1;
            } else if i < n
                && (j == m || lengths[(i + 1) * (m + 1) + j] >= lengths[i * (m + 1) + j + 1])
            {
                rows.push(DiffRow {
                    left: Some(prefix + i),
                    right: None,
                    changed: true,
                });
                i += 1;
            } else {
                rows.push(DiffRow {
                    left: None,
                    right: Some(prefix + j),
                    changed: true,
                });
                j += 1;
            }
        }
    } else {
        // Keep memory and latency bounded; preserve matching ends and mark the middle.
        for i in 0..n.max(m) {
            rows.push(DiffRow {
                left: (i < n).then_some(prefix + i),
                right: (i < m).then_some(prefix + i),
                changed: true,
            });
        }
    }
    for i in 0..a.len() - end_a {
        rows.push(DiffRow {
            left: Some(end_a + i),
            right: Some(end_b + i),
            changed: false,
        });
    }
    cancel
        .check()
        .map_err(|_| "Comparison cancelled".to_owned())?;
    Ok(rows)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_compare_aligns_insertions_and_preserves_final_newlines() {
        let strings = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let a = strings(&["a\n", "c\n"]);
        let b = strings(&["a\n", "b\n", "c\n"]);
        let rows = align(&a, &b, false, false, &CancelToken::new()).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[1].left.is_none());
        assert!(!rows[2].changed);
        assert!(
            align(
                &strings(&["a"]),
                &strings(&["a\n"]),
                false,
                false,
                &CancelToken::new()
            )
            .unwrap()
            .iter()
            .any(|r| r.changed)
        );
        assert!(
            !align(
                &strings(&[" A  b\n"]),
                &strings(&["a b"]),
                true,
                true,
                &CancelToken::new()
            )
            .unwrap()[0]
                .changed
        );
    }
    #[test]
    fn file_compare_detects_binary_and_limits_input() {
        let temp = tempfile::tempdir().unwrap();
        let a = VPath::from(temp.path().join("a"));
        let b = VPath::from(temp.path().join("b"));
        std::fs::write(a.as_path(), [0, 1]).unwrap();
        std::fs::write(b.as_path(), [0, 2]).unwrap();
        let result = load(
            &dualpane_vfs::LocalFs,
            &a,
            &b,
            false,
            false,
            &CancelToken::new(),
        )
        .unwrap();
        assert!(result.binary);
        assert!(result.rows.iter().any(|r| r.changed));
        std::fs::File::create(a.as_path())
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(
            load(
                &dualpane_vfs::LocalFs,
                &a,
                &b,
                false,
                false,
                &CancelToken::new()
            )
            .is_err()
        );
    }
}
