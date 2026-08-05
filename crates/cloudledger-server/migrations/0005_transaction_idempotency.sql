-- Offline clients retain this UUID in their outbox until the server accepts it.
-- A repeated request from the same account and ledger therefore resolves to the
-- originally created transaction instead of creating a duplicate entry.
ALTER TABLE transactions
  ADD COLUMN client_mutation_id UUID;

CREATE UNIQUE INDEX transactions_client_mutation_id_idx
  ON transactions (ledger_id, created_by, client_mutation_id)
  WHERE client_mutation_id IS NOT NULL;
