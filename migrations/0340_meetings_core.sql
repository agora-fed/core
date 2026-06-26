-- 0340_meetings_core — owned by `dsoc-meetings` (range 0340, see migrations/REGISTRY.md).
-- Civic meetings: in-person/online gatherings with minutes and attendance. Explicit,
-- auditable SQL (PLAN.md principle 3 — no ORM, sqlx compile-time-checked queries).
-- PostgreSQL 16+.
--
-- Cross-crate foreign keys point ONLY at the core identity tables `org` and `citizen`
-- (migrations/REGISTRY.md / ARCHITECTURE.md section 3). `meeting_attendee.meeting_id`
-- is an INTRA-crate FK to this crate's own `meeting` table, which is allowed.

-- ---------------------------------------------------------------------------
-- meeting — a scheduled civic gathering. `title` names the meeting, `location`
-- is where it happens (a room, an address, or an online URL), `starts_at` is
-- when it begins, and `minutes` is the (initially absent) record of what was
-- decided, set after the meeting via the set-minutes route.
-- ---------------------------------------------------------------------------
CREATE TABLE meeting (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES org(id),
    title       text NOT NULL CHECK (length(btrim(title)) > 0),
    location    text NOT NULL CHECK (length(btrim(location)) > 0),
    starts_at   timestamptz NOT NULL,
    minutes     text CHECK (minutes IS NULL OR length(btrim(minutes)) > 0),
    created_at  timestamptz NOT NULL
);
-- Keyset pagination over ascending id (UUIDv7 = time-ordered), scoped by org.
CREATE INDEX meeting_org_idx ON meeting (org_id, id);

-- ---------------------------------------------------------------------------
-- meeting_attendee — one citizen's attendance/RSVP for a meeting. `citizen_id`
-- is a cross-crate FK to the core identity table `citizen` (allowed);
-- `meeting_id` is an intra-crate FK. A citizen attends a meeting at most once
-- (UNIQUE), which makes the RSVP write naturally idempotent.
-- ---------------------------------------------------------------------------
CREATE TABLE meeting_attendee (
    id          uuid PRIMARY KEY,
    meeting_id  uuid NOT NULL REFERENCES meeting(id),
    citizen_id  uuid NOT NULL REFERENCES citizen(id),
    created_at  timestamptz NOT NULL,
    UNIQUE (meeting_id, citizen_id)
);
-- Keyset pagination over ascending id within a meeting (oldest-first).
CREATE INDEX meeting_attendee_meeting_idx ON meeting_attendee (meeting_id, id);

COMMENT ON TABLE meeting IS 'Civic meeting with minutes; owned by dsoc-meetings.';
COMMENT ON TABLE meeting_attendee IS 'One citizen attendance/RSVP for a meeting; owned by dsoc-meetings.';
