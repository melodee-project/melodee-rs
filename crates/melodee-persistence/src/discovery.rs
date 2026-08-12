use std::collections::BTreeSet;

use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

pub const MAX_CHART_CSV_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_CHART_ITEMS: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeChartImport {
    pub slug: String,
    pub title: String,
    pub source_name: Option<String>,
    pub source_url: Option<String>,
    pub chart_year: Option<i32>,
    pub is_visible: bool,
    pub is_generated_playlist_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeChartImportResult {
    pub chart_id: Uuid,
    pub item_count: usize,
    pub linked_count: usize,
}

#[derive(Debug, Error)]
pub enum NativeDiscoveryStoreError {
    #[error("invalid chart CSV: {0}")]
    InvalidCsv(String),
    #[error("invalid chart metadata: {0}")]
    InvalidMetadata(&'static str),
    #[error("chart slug already exists")]
    DuplicateSlug,
    #[error("chart persistence failed")]
    Sql(#[source] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChartCsvItem {
    rank: i32,
    artist_name: String,
    release_title: String,
    release_year: Option<i32>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NativeDiscoveryStore;

impl NativeDiscoveryStore {
    /// Imports a bounded chart snapshot and links only unique exact catalog
    /// matches. Ambiguous names stay unlinked because names are never semantic
    /// identity in Melodee.
    pub async fn import_chart_csv(
        &self,
        pool: &PgPool,
        request: &NativeChartImport,
        bytes: &[u8],
    ) -> Result<NativeChartImportResult, NativeDiscoveryStoreError> {
        validate_chart_metadata(request)?;
        let items = parse_chart_csv(bytes)?;
        let chart_id = Uuid::now_v7();
        let mut transaction = pool.begin().await.map_err(map_sql)?;
        let inserted = sqlx::query(
            "INSERT INTO native_charts(\
               id,public_api_id,slug,title,source_name,source_url,chart_year,\
               is_visible,is_generated_playlist_enabled,created_at,updated_at\
             ) VALUES($1,$1,$2,$3,$4,$5,$6,$7,$8,now(),now())",
        )
        .bind(chart_id)
        .bind(request.slug.trim())
        .bind(request.title.trim())
        .bind(request.source_name.as_deref().map(str::trim))
        .bind(request.source_url.as_deref().map(str::trim))
        .bind(request.chart_year)
        .bind(request.is_visible)
        .bind(request.is_generated_playlist_enabled)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = inserted {
            if error
                .as_database_error()
                .is_some_and(|database| database.constraint() == Some("native_charts_slug_key"))
            {
                return Err(NativeDiscoveryStoreError::DuplicateSlug);
            }
            return Err(map_sql(error));
        }

        let ranks = items.iter().map(|item| item.rank).collect::<Vec<_>>();
        let artists = items
            .iter()
            .map(|item| item.artist_name.as_str())
            .collect::<Vec<_>>();
        let releases = items
            .iter()
            .map(|item| item.release_title.as_str())
            .collect::<Vec<_>>();
        let years = items
            .iter()
            .map(|item| item.release_year)
            .collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO native_chart_items(\
               chart_id,rank,artist_name,release_title,release_year,created_at\
             )\
             SELECT $1,item.rank,item.artist_name,item.release_title,item.release_year,now()\
             FROM unnest($2::integer[],$3::text[],$4::text[],$5::integer[]) \
               AS item(rank,artist_name,release_title,release_year)\
             ORDER BY item.rank",
        )
        .bind(chart_id)
        .bind(&ranks)
        .bind(&artists)
        .bind(&releases)
        .bind(&years)
        .execute(&mut *transaction)
        .await
        .map_err(map_sql)?;
        let linked_count = sqlx::query(
            r#"UPDATE native_chart_items item SET
                 (linked_artist_id,linked_release_id)=(
                   SELECT artist.id,release.id
                   FROM artists artist
                   JOIN release_artists credit
                     ON credit.artist_id=artist.id AND credit.role='primary'
                   JOIN releases release
                     ON release.id=credit.release_id AND release.status='active'
                   WHERE lower(btrim(artist.canonical_name))=lower(btrim(item.artist_name))
                     AND lower(btrim(release.title))=lower(btrim(item.release_title))
                     AND (item.release_year IS NULL OR
                       EXTRACT(YEAR FROM release.release_date)::integer=item.release_year)
                   ORDER BY release.id LIMIT 1)
               WHERE item.chart_id=$1 AND 1=(
                 SELECT count(*) FROM artists artist
                 JOIN release_artists credit
                   ON credit.artist_id=artist.id AND credit.role='primary'
                 JOIN releases release
                   ON release.id=credit.release_id AND release.status='active'
                 WHERE lower(btrim(artist.canonical_name))=lower(btrim(item.artist_name))
                   AND lower(btrim(release.title))=lower(btrim(item.release_title))
                   AND (item.release_year IS NULL OR
                     EXTRACT(YEAR FROM release.release_date)::integer=item.release_year))"#,
        )
        .bind(chart_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_sql)?
        .rows_affected();
        transaction.commit().await.map_err(map_sql)?;
        let item_count = items.len();
        let linked_count = usize::try_from(linked_count).map_err(|_| {
            NativeDiscoveryStoreError::InvalidCsv("linked row count overflow".to_owned())
        })?;
        Ok(NativeChartImportResult {
            chart_id,
            item_count,
            linked_count,
        })
    }
}

fn validate_chart_metadata(request: &NativeChartImport) -> Result<(), NativeDiscoveryStoreError> {
    let slug = request.slug.trim();
    if slug.is_empty()
        || slug.len() > 255
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(NativeDiscoveryStoreError::InvalidMetadata(
            "slug must contain 1..255 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    if request.title.trim().is_empty() || request.title.len() > 500 {
        return Err(NativeDiscoveryStoreError::InvalidMetadata(
            "title must contain 1..500 bytes",
        ));
    }
    if request
        .source_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 500)
        || request
            .source_url
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 4096)
    {
        return Err(NativeDiscoveryStoreError::InvalidMetadata(
            "source metadata is empty or exceeds its storage bound",
        ));
    }
    Ok(())
}

fn parse_chart_csv(bytes: &[u8]) -> Result<Vec<ChartCsvItem>, NativeDiscoveryStoreError> {
    if bytes.is_empty() || bytes.len() > MAX_CHART_CSV_BYTES {
        return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
            "input must contain 1..={MAX_CHART_CSV_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| NativeDiscoveryStoreError::InvalidCsv("input must be UTF-8".to_owned()))?;
    let mut items = Vec::new();
    let mut seen_ranks = BTreeSet::new();
    for (line_index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let columns = parse_csv_line(line).map_err(|message| {
            NativeDiscoveryStoreError::InvalidCsv(format!("row {}: {message}", line_index + 1))
        })?;
        if items.is_empty()
            && columns
                .first()
                .is_some_and(|value| value.eq_ignore_ascii_case("rank"))
        {
            continue;
        }
        if columns.len() < 3 || columns.len() > 4 {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: expected Rank, ArtistName, ReleaseTitle, optional ReleaseYear",
                line_index + 1
            )));
        }
        let rank = columns[0].trim().parse::<i32>().map_err(|_| {
            NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: rank must be a positive integer",
                line_index + 1
            ))
        })?;
        if rank < 1 || !seen_ranks.insert(rank) {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: rank must be positive and unique",
                line_index + 1
            )));
        }
        let artist_name = columns[1].trim().to_owned();
        let release_title = columns[2].trim().to_owned();
        if artist_name.is_empty()
            || release_title.is_empty()
            || artist_name.len() > 500
            || release_title.len() > 500
            || artist_name.contains('\0')
            || release_title.contains('\0')
        {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: artist and release names must contain 1..500 bytes",
                line_index + 1
            )));
        }
        let release_year = columns
            .get(3)
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                value.trim().parse::<i32>().map_err(|_| {
                    NativeDiscoveryStoreError::InvalidCsv(format!(
                        "row {}: release year must be an integer",
                        line_index + 1
                    ))
                })
            })
            .transpose()?;
        items.push(ChartCsvItem {
            rank,
            artist_name,
            release_title,
            release_year,
        });
        if items.len() > MAX_CHART_ITEMS {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "input exceeds {MAX_CHART_ITEMS} items"
            )));
        }
    }
    if items.is_empty() {
        return Err(NativeDiscoveryStoreError::InvalidCsv(
            "input contains no chart items".to_owned(),
        ));
    }
    Ok(items)
}

fn parse_csv_line(line: &str) -> Result<Vec<String>, &'static str> {
    let mut columns = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                if let Some(column) = columns.last_mut() {
                    column.push('"');
                }
                let _ = chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => columns.push(String::new()),
            value => {
                if let Some(column) = columns.last_mut() {
                    column.push(value);
                }
            }
        }
    }
    if quoted {
        return Err("unterminated quoted field");
    }
    Ok(columns)
}

fn map_sql(error: sqlx::Error) -> NativeDiscoveryStoreError {
    NativeDiscoveryStoreError::Sql(error)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{MAX_CHART_CSV_BYTES, parse_chart_csv};

    #[test]
    fn chart_csv_parser_handles_headers_quotes_and_rejects_ambiguity() {
        let items = parse_chart_csv(
            b"Rank,ArtistName,ReleaseTitle,ReleaseYear\n1,\"Artist, The\",\"A \"\"Release\"\"\",2020\n",
        )
        .expect("valid CSV");
        assert_eq!(items[0].artist_name, "Artist, The");
        assert_eq!(items[0].release_title, "A \"Release\"");
        assert!(parse_chart_csv(b"1,Artist,Release\n1,Other,Other").is_err());
        assert!(parse_chart_csv(&vec![b'x'; MAX_CHART_CSV_BYTES + 1]).is_err());
    }
}
