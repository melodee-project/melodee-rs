-- ADR-0167: native v1 predicates filter normalized catalog columns. The
-- earlier display-column trigram indexes cannot serve those expressions.
-- Apply under the ADR-0166 maintenance fence: ordinary CREATE INDEX blocks
-- conflicting writes while each catalog-scale index is built.

CREATE INDEX artists_native_search_normalized_name_idx
    ON artists USING gin (normalized_name gin_trgm_ops)
    WHERE status NOT IN ('merged', 'deprecated');

CREATE INDEX artist_aliases_native_search_normalized_alias_idx
    ON artist_aliases USING gin (normalized_alias gin_trgm_ops);

CREATE INDEX releases_native_search_normalized_title_idx
    ON releases USING gin (normalized_title gin_trgm_ops)
    WHERE status = 'active';

CREATE INDEX tracks_native_search_normalized_title_idx
    ON tracks USING gin (normalized_title gin_trgm_ops);
