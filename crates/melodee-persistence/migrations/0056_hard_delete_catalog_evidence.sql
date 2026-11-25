-- ADR-0211: explicit hard deletion may remove catalog evidence rows.
--
-- The evidence tables are append-only for ordinary repository code. A hard
-- deletion requested by an administrator or editor is the one exceptional
-- operation allowed to remove a Release's evidence, and it runs in a
-- transaction that first sets the local `melodee.hard_delete` flag. The
-- triggers still reject every other UPDATE/DELETE path, including direct SQL
-- that forgets the flag.
--
-- Additive and reversible: no row is changed by this migration. Rolling back
-- restores the previous trigger functions; rows removed by an authorized hard
-- deletion are not restored by a database rollback.

CREATE OR REPLACE FUNCTION reject_catalog_evidence_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('melodee.hard_delete', true) = 'on' THEN
        IF TG_OP = 'DELETE' THEN
            RETURN OLD;
        END IF;
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'catalog evidence table % is append-only', TG_TABLE_NAME;
END;
$$;

CREATE OR REPLACE FUNCTION reject_policy_fact_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('melodee.hard_delete', true) = 'on' THEN
        IF TG_OP = 'DELETE' THEN
            RETURN OLD;
        END IF;
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'classification and novelty facts are immutable';
END;
$$;

CREATE OR REPLACE FUNCTION reject_policy_decision_audit_rewrite() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('melodee.hard_delete', true) = 'on' THEN
        IF TG_OP = 'DELETE' THEN
            RETURN OLD;
        END IF;
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'policy decision audit rows are append-only';
END;
$$;

CREATE OR REPLACE FUNCTION reject_policy_decision_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('melodee.hard_delete', true) = 'on' THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'policy decisions are durable and cannot be deleted';
END;
$$;
