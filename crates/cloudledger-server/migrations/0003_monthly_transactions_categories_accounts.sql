ALTER TABLE financial_accounts
  DROP CONSTRAINT financial_accounts_kind_check;

ALTER TABLE financial_accounts
  ADD CONSTRAINT financial_accounts_kind_check
  CHECK (kind IN (
    'cash', 'bank', 'wallet', 'wechat', 'alipay',
    'credit', 'receivable', 'payable'
  ));

UPDATE financial_accounts
SET kind = 'wechat', name = '微信'
WHERE kind = 'wallet';

UPDATE financial_accounts SET name = '银行账户' WHERE kind = 'bank';
UPDATE financial_accounts SET name = '现金' WHERE kind = 'cash';

INSERT INTO financial_accounts (
  id, ledger_id, name, kind, opening_balance_minor, currency, created_at, deleted_at
)
SELECT
  gen_random_uuid(),
  ledger.id,
  standard_account.name,
  standard_account.kind,
  0,
  COALESCE(existing_currency.currency, 'CNY'),
  NOW(),
  NULL
FROM ledgers AS ledger
CROSS JOIN (
  VALUES
    ('微信', 'wechat'),
    ('支付宝', 'alipay'),
    ('银行账户', 'bank'),
    ('现金', 'cash')
) AS standard_account(name, kind)
LEFT JOIN LATERAL (
  SELECT account.currency
  FROM financial_accounts AS account
  WHERE account.ledger_id = ledger.id
  ORDER BY account.created_at, account.id
  LIMIT 1
) AS existing_currency ON TRUE
WHERE ledger.deleted_at IS NULL
  AND NOT EXISTS (
    SELECT 1
    FROM financial_accounts AS account
    WHERE account.ledger_id = ledger.id
      AND account.kind = standard_account.kind
      AND account.deleted_at IS NULL
  );

CREATE TABLE categories (
  id UUID PRIMARY KEY,
  ledger_id UUID NOT NULL REFERENCES ledgers(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  CHECK (kind IN ('income', 'expense')),
  CHECK (char_length(btrim(name)) BETWEEN 1 AND 24)
);

CREATE UNIQUE INDEX categories_ledger_kind_name_uq
  ON categories (ledger_id, kind, lower(name));

INSERT INTO categories (id, ledger_id, name, kind)
SELECT gen_random_uuid(), ledger.id, default_category.name, default_category.kind
FROM ledgers AS ledger
CROSS JOIN (
  VALUES
    ('餐饮', 'expense'),
    ('交通', 'expense'),
    ('办公', 'expense'),
    ('采购', 'expense'),
    ('差旅', 'expense'),
    ('其他支出', 'expense'),
    ('工资', 'income'),
    ('业务收入', 'income'),
    ('其他收入', 'income')
) AS default_category(name, kind)
WHERE ledger.deleted_at IS NULL;

UPDATE transactions AS transaction
SET category_id = category.id
FROM categories AS category
WHERE transaction.category_id IS NULL
  AND category.ledger_id = transaction.ledger_id
  AND category.kind = transaction.kind
  AND category.name = CASE transaction.kind
    WHEN 'income' THEN '其他收入'
    WHEN 'expense' THEN '其他支出'
  END;

ALTER TABLE transactions
  ADD CONSTRAINT transactions_category_id_fkey
    FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE RESTRICT,
  ADD CONSTRAINT transactions_category_required_check
    CHECK (kind = 'transfer' OR category_id IS NOT NULL);

UPDATE app_metadata SET schema_version = 3 WHERE schema_version < 3;
