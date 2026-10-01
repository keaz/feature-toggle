-- System client shadow users were created with is_admin = TRUE, which let machine
-- tokens pass human-admin checks. They keep the Requester/Approver roles only.
UPDATE users SET is_admin = FALSE WHERE id IN (SELECT id FROM system_clients);
