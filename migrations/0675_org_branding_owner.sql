-- 0675 — org_branding ownership for the production role.
--
-- Production applies migrations as the postgres superuser while the gateway
-- connects as role `dsoc` (same situation as 0508/0513/0532): without this,
-- the branding upsert dies with permission denied. No-op wherever the
-- migrating role IS dsoc (dev compose, CI).

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dsoc') THEN
        ALTER TABLE org_branding OWNER TO dsoc;
    END IF;
END $$;
