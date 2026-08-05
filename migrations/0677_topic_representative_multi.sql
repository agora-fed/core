-- 0677 — multiple representatives per citizen per topic (product decision
-- 2026-08-05, follow-up of 0676/issue #3).
--
-- A citizen may now mark SEVERAL mandates on one cause (e.g. their whole
-- caucus), each at most once. The API caps the count per citizen per topic
-- (5 today) so "tag everyone" noise cannot dilute the ranking; the cap is an
-- application constant, not a schema rule.

ALTER TABLE topic_representative_tag
    DROP CONSTRAINT topic_representative_tag_org_id_topic_id_citizen_id_key;

ALTER TABLE topic_representative_tag
    ADD CONSTRAINT topic_representative_tag_citizen_mandate_key
    UNIQUE (org_id, topic_id, citizen_id, mandate_id);
