import {
  createTransaction as invokeCreateTransaction,
  loadOverview,
  type AccountDto,
  type AuditLogDto,
  type LedgerDto,
  type LedgerOverview,
  type TransactionDto,
} from "../api";
import type {
  ApprovalQueueItem,
  AuditLogEntry,
  Category,
  FinancialAccount,
  Ledger,
  LedgerDashboard,
  NewTransactionDraft,
  Transaction,
} from "../types";

export interface CloudLedgerApi {
  listLedgers(): Promise<Ledger[]>;
  getLedgerDashboard(ledgerId: string): Promise<LedgerDashboard>;
  createTransaction(draft: NewTransactionDraft): Promise<Transaction>;
  listApprovalQueue(ledgerId: string): Promise<ApprovalQueueItem[]>;
  listAuditTrail(ledgerId: string): Promise<AuditLogEntry[]>;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const isTauriRuntime = () => typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);

let overviewCache: LedgerOverview | undefined;

const getOverview = async () => {
  overviewCache = await loadOverview();
  return overviewCache;
};

const commandApi: CloudLedgerApi = {
  async listLedgers() {
    const overview = await getOverview();
    return overview.ledgers.map((ledger) => mapLedger(ledger, overview));
  },

  async getLedgerDashboard(ledgerId) {
    const overview = await getOverview();
    return mapDashboard(ledgerId, overview);
  },

  async createTransaction(draft) {
    const overview = overviewCache ?? (await getOverview());
    const account = overview.accounts.find((item) => item.id === draft.accountId);
    const category = categoryFromDraft(draft, overview);
    const ledger = overview.ledgers.find((item) => item.id === draft.ledgerId);
    const created = await invokeCreateTransaction({
      actorUserId: overview.currentUser.id,
      ledgerId: draft.ledgerId,
      accountId: draft.accountId,
      kind: draft.direction,
      amountMinor: draft.amountCents,
      currency: account?.currency ?? "CNY",
      description: draft.memo?.trim() || category?.name || ledger?.name || "未命名流水",
    });

    overviewCache = undefined;
    return mapTransaction(created, overview);
  },

  async listApprovalQueue(ledgerId) {
    const overview = await getOverview();
    return mapApprovalQueue(ledgerId, overview);
  },

  async listAuditTrail(ledgerId) {
    const overview = await getOverview();
    return overview.auditLogs.filter((item) => item.ledgerId === ledgerId).map(mapAuditLog);
  },
};

const nowIso = () => new Date().toISOString();

const mockLedgers: Ledger[] = [
  {
    id: "personal-main",
    name: "个人账本",
    kind: "private",
    currency: "CNY",
    role: "owner",
    balanceCents: 186420,
    pendingCount: 0,
    auditUnreadCount: 0,
    lastSyncedAt: nowIso(),
  },
  {
    id: "org-growth",
    name: "增长事业部",
    kind: "organization",
    currency: "CNY",
    role: "accountant",
    organizationName: "CloudLedger Inc.",
    balanceCents: 8429500,
    pendingCount: 3,
    auditUnreadCount: 8,
    lastSyncedAt: nowIso(),
  },
];

const mockAccounts: FinancialAccount[] = [
  {
    id: "cash",
    ledgerId: "personal-main",
    name: "现金",
    kind: "cash",
    balanceCents: 34200,
  },
  {
    id: "wallet",
    ledgerId: "personal-main",
    name: "电子钱包",
    kind: "wallet",
    balanceCents: 152220,
  },
  {
    id: "company-bank",
    ledgerId: "org-growth",
    name: "公司基本户",
    kind: "company",
    balanceCents: 6237000,
  },
  {
    id: "receivable",
    ledgerId: "org-growth",
    name: "应收账款",
    kind: "receivable",
    balanceCents: 2192500,
  },
];

const mockCategories: Category[] = [
  { id: "meals", ledgerId: "personal-main", name: "餐饮", direction: "expense" },
  { id: "transport", ledgerId: "personal-main", name: "交通", direction: "expense" },
  { id: "salary", ledgerId: "personal-main", name: "工资", direction: "income" },
  { id: "cloud", ledgerId: "org-growth", name: "云资源", direction: "expense" },
  { id: "travel", ledgerId: "org-growth", name: "差旅", direction: "expense" },
  { id: "contract", ledgerId: "org-growth", name: "合同回款", direction: "income" },
];

let mockTransactions: Transaction[] = [
  {
    id: "tx-101",
    ledgerId: "personal-main",
    occurredAt: "2026-07-08T08:45:00.000Z",
    title: "早餐",
    amountCents: 1800,
    direction: "expense",
    accountName: "电子钱包",
    categoryName: "餐饮",
    approvalState: "approved",
    actorName: "我",
    memo: "移动端快速记账",
    auditRequired: false,
  },
  {
    id: "tx-102",
    ledgerId: "personal-main",
    occurredAt: "2026-07-07T13:10:00.000Z",
    title: "地铁",
    amountCents: 600,
    direction: "expense",
    accountName: "电子钱包",
    categoryName: "交通",
    approvalState: "approved",
    actorName: "我",
    auditRequired: false,
  },
  {
    id: "tx-201",
    ledgerId: "org-growth",
    occurredAt: "2026-07-08T02:20:00.000Z",
    title: "对象存储月账单",
    amountCents: 98600,
    direction: "expense",
    accountName: "公司基本户",
    categoryName: "云资源",
    approvalState: "pending",
    actorName: "林会计",
    memo: "待主管确认",
    auditRequired: true,
  },
  {
    id: "tx-202",
    ledgerId: "org-growth",
    occurredAt: "2026-07-06T10:00:00.000Z",
    title: "企业客户回款",
    amountCents: 320000,
    direction: "income",
    accountName: "应收账款",
    categoryName: "合同回款",
    approvalState: "approved",
    actorName: "陈经理",
    auditRequired: true,
  },
];

let mockApprovalQueue: ApprovalQueueItem[] = [
  {
    id: "approval-201",
    transactionId: "tx-201",
    ledgerId: "org-growth",
    title: "对象存储月账单",
    amountCents: 98600,
    direction: "expense",
    submittedBy: "林会计",
    submittedAt: "2026-07-08T02:25:00.000Z",
    state: "pending",
  },
  {
    id: "approval-203",
    transactionId: "tx-203",
    ledgerId: "org-growth",
    title: "差旅报销",
    amountCents: 42680,
    direction: "expense",
    submittedBy: "周运营",
    submittedAt: "2026-07-07T09:30:00.000Z",
    state: "pending",
  },
];

let mockAuditTrail: AuditLogEntry[] = [
  {
    id: "audit-301",
    ledgerId: "org-growth",
    action: "transaction_submitted",
    actorName: "林会计",
    createdAt: "2026-07-08T02:25:00.000Z",
    summary: "提交对象存储月账单审批",
  },
  {
    id: "audit-302",
    ledgerId: "org-growth",
    action: "transaction_approved",
    actorName: "陈经理",
    createdAt: "2026-07-06T10:08:00.000Z",
    summary: "批准企业客户回款入账",
  },
];

const mockApi: CloudLedgerApi = {
  async listLedgers() {
    return mockLedgers;
  },

  async getLedgerDashboard(ledgerId) {
    const ledger = mockLedgers.find((item) => item.id === ledgerId) ?? mockLedgers[0];
    return {
      ledger,
      accounts: mockAccounts.filter((item) => item.ledgerId === ledger.id),
      categories: mockCategories.filter((item) => item.ledgerId === ledger.id),
      recentTransactions: mockTransactions
        .filter((item) => item.ledgerId === ledger.id)
        .sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt)),
      approvalQueue: mockApprovalQueue.filter((item) => item.ledgerId === ledger.id),
      auditTrail: mockAuditTrail.filter((item) => item.ledgerId === ledger.id),
    };
  },

  async createTransaction(draft) {
    const account = mockAccounts.find((item) => item.id === draft.accountId);
    const category = mockCategories.find((item) => item.id === draft.categoryId);
    const transaction: Transaction = {
      id: crypto.randomUUID(),
      ledgerId: draft.ledgerId,
      occurredAt: draft.occurredAt,
      title: draft.memo?.trim() || category?.name || "未命名流水",
      amountCents: draft.amountCents,
      direction: draft.direction,
      accountName: account?.name ?? "未选择账户",
      categoryName: category?.name ?? "未分类",
      approvalState: draft.submitForApproval ? "pending" : "draft",
      actorName: "我",
      memo: draft.memo,
      auditRequired: draft.submitForApproval,
    };

    mockTransactions = [transaction, ...mockTransactions];

    if (draft.submitForApproval) {
      mockApprovalQueue = [
        {
          id: crypto.randomUUID(),
          transactionId: transaction.id,
          ledgerId: draft.ledgerId,
          title: transaction.title,
          amountCents: draft.amountCents,
          direction: draft.direction,
          submittedBy: "我",
          submittedAt: nowIso(),
          state: "pending",
        },
        ...mockApprovalQueue,
      ];
      mockAuditTrail = [
        {
          id: crypto.randomUUID(),
          ledgerId: draft.ledgerId,
          action: "transaction_submitted",
          actorName: "我",
          createdAt: nowIso(),
          summary: `提交${transaction.title}审批`,
        },
        ...mockAuditTrail,
      ];
    }

    return transaction;
  },

  async listApprovalQueue(ledgerId) {
    return mockApprovalQueue.filter((item) => item.ledgerId === ledgerId);
  },

  async listAuditTrail(ledgerId) {
    return mockAuditTrail.filter((item) => item.ledgerId === ledgerId);
  },
};

export const cloudLedgerApi: CloudLedgerApi = isTauriRuntime() ? commandApi : mockApi;

function mapDashboard(ledgerId: string, overview: LedgerOverview): LedgerDashboard {
  const ledgerDto = overview.ledgers.find((item) => item.id === ledgerId) ?? overview.ledgers[0];
  const ledger = mapLedger(ledgerDto, overview);
  const accounts = overview.accounts.filter((item) => item.ledgerId === ledger.id).map(mapAccount);
  const categories = buildCategories(ledger.id, overview);
  const recentTransactions = overview.transactions
    .filter((item) => item.ledgerId === ledger.id)
    .map((item) => mapTransaction(item, overview))
    .sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt));

  return {
    ledger,
    accounts,
    categories,
    recentTransactions,
    approvalQueue: mapApprovalQueue(ledger.id, overview),
    auditTrail: overview.auditLogs.filter((item) => item.ledgerId === ledger.id).map(mapAuditLog),
  };
}

function mapLedger(ledger: LedgerDto, overview: LedgerOverview): Ledger {
  const accounts = overview.accounts.filter((item) => item.ledgerId === ledger.id);
  const balanceCents = accounts.reduce((total, account) => total + account.balanceMinor, 0);
  const transactions = overview.transactions.filter((item) => item.ledgerId === ledger.id);
  const latestTransaction = transactions
    .map((item) => item.occurredAt)
    .sort((a, b) => Date.parse(b) - Date.parse(a))[0];

  return {
    id: ledger.id,
    name: ledger.name,
    kind: ledger.kind === "personal" ? "private" : "organization",
    currency: accounts[0]?.currency ?? "CNY",
    role: ledger.kind === "personal" ? "owner" : "accountant",
    organizationName: ledger.kind === "organization_public" ? ledger.scopeLabel : undefined,
    balanceCents,
    pendingCount: transactions.filter((item) => item.approvalState === "submitted").length,
    auditUnreadCount: overview.auditLogs.filter((item) => item.ledgerId === ledger.id).length,
    lastSyncedAt: latestTransaction,
  };
}

function mapAccount(account: AccountDto): FinancialAccount {
  return {
    id: account.id,
    ledgerId: account.ledgerId,
    name: account.name,
    kind: normalizeAccountKind(account.kind),
    balanceCents: account.balanceMinor,
  };
}

function buildCategories(ledgerId: string, overview: LedgerOverview): Category[] {
  const directions = new Set(
    overview.transactions.filter((item) => item.ledgerId === ledgerId).map((item) => item.kind),
  );
  const defaults: Category[] = [
    { id: `${ledgerId}:expense`, ledgerId, name: "支出", direction: "expense" },
    { id: `${ledgerId}:income`, ledgerId, name: "收入", direction: "income" },
  ];

  if (directions.has("transfer")) {
    defaults.push({ id: `${ledgerId}:transfer`, ledgerId, name: "转账", direction: "expense" });
  }

  return defaults;
}

function mapTransaction(transaction: TransactionDto, overview: LedgerOverview): Transaction {
  const account = overview.accounts.find((item) => item.id === transaction.accountId);

  return {
    id: transaction.id,
    ledgerId: transaction.ledgerId,
    occurredAt: transaction.occurredAt,
    title: transaction.description,
    amountCents: transaction.amountMinor,
    direction: transaction.kind === "income" ? "income" : "expense",
    accountName: account?.name ?? "未知账户",
    categoryName: transaction.kind === "income" ? "收入" : "支出",
    approvalState: normalizeApprovalState(transaction.approvalState),
    actorName: transaction.createdBy,
    memo: transaction.description,
    auditRequired: transaction.approvalState === "submitted",
  };
}

function mapApprovalQueue(ledgerId: string, overview: LedgerOverview): ApprovalQueueItem[] {
  return overview.transactions
    .filter((item) => item.ledgerId === ledgerId && item.approvalState === "submitted")
    .map((transaction) => ({
      id: `${transaction.id}:approval`,
      transactionId: transaction.id,
      ledgerId: transaction.ledgerId,
      title: transaction.description,
      amountCents: transaction.amountMinor,
      direction: transaction.kind === "income" ? "income" : "expense",
      submittedBy: transaction.createdBy,
      submittedAt: transaction.occurredAt,
      state: "pending",
    }));
}

function mapAuditLog(entry: AuditLogDto): AuditLogEntry {
  return {
    id: entry.id,
    ledgerId: entry.ledgerId,
    action: normalizeAuditAction(entry.action),
    actorName: entry.actorUserId,
    createdAt: entry.createdAt,
    summary: entry.summary,
  };
}

function categoryFromDraft(draft: NewTransactionDraft, overview: LedgerOverview): Category | undefined {
  return buildCategories(draft.ledgerId, overview).find((category) => category.id === draft.categoryId);
}

function normalizeAccountKind(kind: string): FinancialAccount["kind"] {
  if (
    kind === "cash" ||
    kind === "bank" ||
    kind === "wallet" ||
    kind === "company" ||
    kind === "receivable" ||
    kind === "payable"
  ) {
    return kind;
  }

  return "bank";
}

function normalizeApprovalState(state: TransactionDto["approvalState"]): Transaction["approvalState"] {
  const states: Record<TransactionDto["approvalState"], Transaction["approvalState"]> = {
    draft: "draft",
    submitted: "pending",
    approved: "approved",
    rejected: "rejected",
    voided: "deleted",
  };

  return states[state];
}

function normalizeAuditAction(action: string): AuditLogEntry["action"] {
  if (action.includes("approved")) {
    return "transaction_approved";
  }

  if (action.includes("rejected")) {
    return "transaction_rejected";
  }

  if (action.includes("deleted") || action.includes("voided")) {
    return "transaction_deleted";
  }

  if (action.includes("submitted")) {
    return "transaction_submitted";
  }

  return "transaction_created";
}
