//! Saved automatic and reviewed identity links for one installation.
use super::*;
use crate::commands::common::read_bounded_file;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MapFile {
    schema_version: u32,
    target_installation_id: Uuid,
    approved: Vec<Link>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Link {
    kind: String,
    legacy_id: Uuid,
    target_id: Uuid,
}

type Pairs = BTreeMap<Uuid, Uuid>;
type LinksByKind = BTreeMap<String, Pairs>;

fn read_map(
    path: &Path,
    installation: Uuid,
    required: bool,
) -> Result<(Option<String>, LinksByKind), String> {
    let bytes = match read_bounded_file(path, 128 * 1024 * 1024) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound && !required => {
            return Ok((None, BTreeMap::new()));
        }
        Err(error) => {
            return Err(format!(
                "cannot read identity map {}: {error}",
                path.display()
            ));
        }
    };
    let file: MapFile =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid identity map: {error}"))?;
    if file.schema_version != 1 {
        return Err("unsupported identity map version".into());
    }
    if installation != file.target_installation_id {
        return Err("identity map belongs to a different target installation".into());
    }
    let mut links = LinksByKind::new();
    let mut targets = BTreeSet::new();
    for link in file.approved {
        if !matches!(link.kind.as_str(), "user" | "artist" | "release" | "track")
            || link.legacy_id.is_nil()
            || link.target_id.is_nil()
            || link.legacy_id == link.target_id
        {
            return Err(
                "an approved link must have a known kind and two distinct, nonempty public IDs"
                    .into(),
            );
        }
        if !targets.insert((link.kind.clone(), link.target_id))
            || links
                .entry(link.kind)
                .or_default()
                .insert(link.legacy_id, link.target_id)
                .is_some()
        {
            return Err("approved links must be one-to-one within each kind".into());
        }
    }
    Ok((Some(sha256_hex(&bytes)), links))
}

struct SavedLinks {
    kind: &'static str,
    path: PathBuf,
    original_digest: Option<String>,
    pairs: Pairs,
    changed: bool,
}

impl SavedLinks {
    fn load(path: PathBuf, installation: Uuid, kind: &'static str) -> Result<Self, String> {
        let (original_digest, mut links) = read_map(&path, installation, false)?;
        let pairs = links.remove(kind).unwrap_or_default();
        if !links.is_empty() {
            return Err(format!(
                "saved {kind} link file contains an unexpected identity type"
            ));
        }
        Ok(Self {
            kind,
            path,
            original_digest,
            pairs,
            changed: false,
        })
    }

    fn verify(&self) -> Result<(), String> {
        let digest = match read_bounded_file(&self.path, 128 * 1024 * 1024) {
            Ok(bytes) => Some(sha256_hex(&bytes)),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(format!("cannot check identity map: {err}")),
        };
        if digest != self.original_digest {
            return Err(
                "identity map changed during import planning; rerun to use the saved links".into(),
            );
        }
        Ok(())
    }

    fn save(&self, parent: &Path, installation: Uuid) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
        let map = MapFile {
            schema_version: 1,
            target_installation_id: installation,
            approved: self
                .pairs
                .iter()
                .map(|(&legacy_id, &target_id)| Link {
                    kind: self.kind.to_owned(),
                    legacy_id,
                    target_id,
                })
                .collect(),
        };
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut tmp, &map).map_err(|e| e.to_string())?;
        tmp.write_all(b"\n")
            .and_then(|()| tmp.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        tmp.persist(&self.path)
            .map_err(|e| format!("cannot save {} links: {e}", self.kind))?;
        Ok(())
    }
}

pub(super) struct IdentityMap {
    installation: Uuid,
    directory: PathBuf,
    accounts: SavedLinks,
    tracks: SavedLinks,
    by_kind: LinksByKind,
}

impl IdentityMap {
    pub(super) async fn load(
        pool: &PgPool,
        input: &Path,
        path: Option<&Path>,
    ) -> Result<Self, String> {
        let installation = melodee_persistence::installation_id(pool)
            .await
            .map_err(|e| format!("cannot identify target installation: {e}"))?;
        Self::open(input, installation, path)
    }

    fn open(input: &Path, installation: Uuid, path: Option<&Path>) -> Result<Self, String> {
        let accounts = SavedLinks::load(
            input.join(format!("account-links-{installation}.json")),
            installation,
            "user",
        )?;
        let tracks = SavedLinks::load(
            input.join(format!("track-links-{installation}.json")),
            installation,
            "track",
        )?;
        let mut by_kind = LinksByKind::from([
            ("user".into(), accounts.pairs.clone()),
            ("track".into(), tracks.pairs.clone()),
        ]);
        if let Some(path) = path {
            let (_, manual) = read_map(path, installation, true)?;
            for (kind, pairs) in manual {
                let links = by_kind.entry(kind).or_default();
                let mut used: BTreeSet<_> = links.values().copied().collect();
                for (source, target) in pairs {
                    if links.get(&source) == Some(&target) {
                        continue;
                    }
                    if links.insert(source, target).is_some() || !used.insert(target) {
                        return Err(
                            "reviewed link conflicts with a previously saved identity".into()
                        );
                    }
                }
            }
        }
        Ok(Self {
            installation,
            directory: input.to_owned(),
            accounts,
            tracks,
            by_kind,
        })
    }

    pub(super) fn targets(&self, kind: &str) -> BTreeSet<Uuid> {
        self.by_kind
            .get(kind)
            .into_iter()
            .flat_map(|links| links.values().copied())
            .collect()
    }

    pub(super) fn link_email_match(&mut self, source: Uuid, target: Uuid) {
        Self::remember(&mut self.accounts, &mut self.by_kind, source, target);
    }

    pub(super) fn link_track_match(&mut self, source: Uuid, target: Uuid) {
        Self::remember(&mut self.tracks, &mut self.by_kind, source, target);
    }

    fn remember(file: &mut SavedLinks, by_kind: &mut LinksByKind, source: Uuid, target: Uuid) {
        file.pairs.insert(source, target);
        file.changed = true;
        by_kind
            .entry(file.kind.into())
            .or_default()
            .insert(source, target);
    }

    /// Validate both plans under the same directory lock before replacing a receipt.
    /// Each file is independently usable if a later write or the database import fails.
    pub(super) fn save(&self) -> Result<(), String> {
        if !self.accounts.changed && !self.tracks.changed {
            return Ok(());
        }
        let dir = std::fs::File::open(&self.directory).map_err(|e| e.to_string())?;
        dir.try_lock().map_err(|e|format!("cannot lock the export directory to save links: {e}; retry after the current import"))?;
        self.accounts.verify()?;
        self.tracks.verify()?;
        self.accounts.save(&self.directory, self.installation)?;
        self.tracks.save(&self.directory, self.installation)?;
        dir.sync_all().map_err(|e| e.to_string())
    }

    pub(super) fn target(&self, kind: &str, source: &Uuid) -> Option<Uuid> {
        self.by_kind.get(kind)?.get(source).copied()
    }

    /// Return internal IDs keyed by the original export IDs. In particular, play event
    /// deduplication continues to use the same source IDs on every retry.
    pub(super) async fn resolve(
        &self,
        pool: &PgPool,
        kind: &str,
        sources: BTreeSet<Uuid>,
        query: &'static str,
    ) -> Result<BTreeMap<Uuid, Uuid>, String> {
        if sources.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut lookup = sources.clone();
        for source in &sources {
            if let Some(target) = self.target(kind, source) {
                if sources.contains(&target) {
                    return Err(format!(
                        "approved {kind} target {target} is also a separate export identity"
                    ));
                }
                lookup.insert(target);
            }
        }
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(query)
            .bind(lookup.into_iter().collect::<Vec<_>>())
            .fetch_all(pool)
            .await
            .map_err(|error| format!("{kind} lookup failed: {error}"))?;
        let indexed: BTreeMap<_, _> = rows.into_iter().collect();
        let mut resolved = BTreeMap::new();
        for source in sources {
            let target = self.target(kind, &source).unwrap_or(source);
            if target != source && indexed.contains_key(&source) {
                return Err(format!(
                    "approved {kind} link conflicts with an existing public ID {source}"
                ));
            }
            if let Some(internal) = indexed.get(&target) {
                resolved.insert(source, *internal);
            } else if target != source && kind == "user" {
                return Err(format!(
                    "approved user target {target} does not exist on this server"
                ));
            }
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn stale_import_plan_cannot_replace_saved_email_links() {
        let dir = tempfile::tempdir().expect("private export");
        let installation = Uuid::now_v7();
        let source = Uuid::now_v7();
        let original = Uuid::now_v7();
        let mut first = IdentityMap::open(dir.path(), installation, None).expect("first plan");
        let mut second =
            IdentityMap::open(dir.path(), installation, None).expect("overlapping plan");

        first.link_email_match(source, original);
        second.link_email_match(source, Uuid::now_v7());
        first.save().expect("first decision saved");
        assert!(second.save().is_err());

        let saved = IdentityMap::open(dir.path(), installation, None).expect("saved binding");
        assert_eq!(saved.target("user", &source), Some(original));
    }
}
