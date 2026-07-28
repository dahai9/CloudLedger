ALTER TABLE organization_memberships
  DROP CONSTRAINT organization_memberships_role_check;

UPDATE organization_memberships
SET role = CASE role
  WHEN 'approver' THEN 'business_owner'
  WHEN 'accountant' THEN 'employee'
  WHEN 'member' THEN 'employee'
  WHEN 'viewer' THEN 'employee'
  ELSE role
END;

ALTER TABLE organization_memberships
  ADD CONSTRAINT organization_memberships_role_check
  CHECK (role IN ('owner', 'admin', 'business_owner', 'employee'));

ALTER TABLE transactions
  ADD COLUMN payment_state TEXT NOT NULL DEFAULT 'not_applicable',
  ADD COLUMN approved_at TIMESTAMPTZ,
  ADD COLUMN paid_by UUID REFERENCES domain_users(id) ON DELETE RESTRICT,
  ADD COLUMN paid_at TIMESTAMPTZ,
  ADD COLUMN received_by UUID REFERENCES domain_users(id) ON DELETE RESTRICT,
  ADD COLUMN received_at TIMESTAMPTZ;

UPDATE transactions
SET payment_state = CASE
      WHEN kind = 'expense' AND approval_state = 'approved' THEN 'received'
      ELSE 'not_applicable'
    END,
    approved_at = CASE
      WHEN approval_state = 'approved' THEN updated_at
      ELSE NULL
    END,
    received_by = CASE
      WHEN kind = 'expense' AND approval_state = 'approved' THEN created_by
      ELSE NULL
    END,
    received_at = CASE
      WHEN kind = 'expense' AND approval_state = 'approved' THEN updated_at
      ELSE NULL
    END;

ALTER TABLE transactions
  ADD CONSTRAINT transactions_payment_state_check
  CHECK (payment_state IN ('not_applicable', 'pending_payment', 'paid_pending_receipt', 'received')),
  ADD CONSTRAINT transactions_payment_timestamps_check
  CHECK (
    (payment_state = 'not_applicable' AND paid_by IS NULL AND paid_at IS NULL AND received_by IS NULL AND received_at IS NULL)
    OR
    (payment_state = 'pending_payment' AND paid_by IS NULL AND paid_at IS NULL AND received_by IS NULL AND received_at IS NULL)
    OR
    (payment_state = 'paid_pending_receipt' AND paid_by IS NOT NULL AND paid_at IS NOT NULL AND received_by IS NULL AND received_at IS NULL)
    OR
    (payment_state = 'received' AND received_by IS NOT NULL AND received_at IS NOT NULL)
  );

CREATE INDEX transactions_payment_idx
  ON transactions (ledger_id, payment_state);
