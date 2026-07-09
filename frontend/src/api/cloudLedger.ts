import {
  createTransaction as invokeCreateTransaction,
  decideApproval as invokeDecideApproval,
  loadOverview,
  switchUser as invokeSwitchUser,
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
  UserSession,
} from "../types";

export interface CloudLedgerApi {
  getUserSession(): Promise<UserSession>;
  switchUser(userId: string): Promise<void>;
  listLedgers(): Promise<Ledger[]>;
  getLedgerDashboard(ledgerId: string): Promise<LedgerDashboard>;
  createTransaction(draft: NewTransactionDraft): Promise<Transaction>;
  decideApproval(
    transactionId: string,
    decision: "approve" | "reject",
    decisionNote?: string,
  ): Promise<Transaction>;
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
const cloudBaseUrl = import.meta.env.VITE_CLOUDLEDGER_CLOUD_URL ?? "http://192.168.1.32:8787";

const getOverview = async () => {
  overviewCache = await loadOverview();
  return overviewCache;
};

const commandApi: CloudLedgerApi = {
  async getUserSession() {
    const overview = await getOverview();
    return {
      currentUser: overview.currentUser,
      users: overview.users,
      cloudStatus: await fetchCloudStatus(),
    };
  },

  async switchUser(userId) {
    overviewCache = await invokeSwitchUser(userId);
  },

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

  async decideApproval(transactionId, decision, decisionNote) {
    const overview = overviewCache ?? (await getOverview());
    const decided = await invokeDecideApproval({ transactionId, decision, decisionNote });
    overviewCache = undefined;
    return mapTransaction(decided, overview);
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
    submittedById: "demo-lin",
    submittedAt: "2026-07-08T02:25:00.000Z",
    state: "pending",
    canDecide: true,
  },
  {
    id: "approval-203",
    transactionId: "tx-203",
    ledgerId: "org-growth",
    title: "差旅报销",
    amountCents: 42680,
    direction: "expense",
    submittedBy: "周运营",
    submittedById: "demo-zhou",
    submittedAt: "2026-07-07T09:30:00.000Z",
    state: "pending",
    canDecide: true,
  },
];

let mockAuditTrail: AuditLogEntry[] = [
  {
    id: "audit-301",
    ledgerId: "org-growth",
    action: "transaction_submitted",
    actorName: "林会计",
    resourceId: "tx-201",
    createdAt: "2026-07-08T02:25:00.000Z",
    summary: "提交对象存储月账单审批",
  },
  {
    id: "audit-302",
    ledgerId: "org-growth",
    action: "transaction_approved",
    actorName: "陈经理",
    resourceId: "tx-202",
    createdAt: "2026-07-06T10:08:00.000Z",
    summary: "批准企业客户回款入账",
  },
];

const mockApi: CloudLedgerApi = {
  async getUserSession() {
    return {
      currentUser: { id: "demo-user", displayName: "Alice" },
      users: [
        { id: "demo-user", displayName: "Alice" },
        { id: "demo-bob", displayName: "Bob" },
      ],
      cloudStatus: await fetchCloudStatus(),
    };
  },

  async switchUser() {
    return undefined;
  },

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
          submittedById: "demo-user",
          submittedAt: nowIso(),
          state: "pending",
          canDecide: false,
        },
        ...mockApprovalQueue,
      ];
      mockAuditTrail = [
        {
          id: crypto.randomUUID(),
          ledgerId: draft.ledgerId,
          action: "transaction_submitted",
          actorName: "我",
          resourceId: transaction.id,
          createdAt: nowIso(),
          summary: `提交${transaction.title}审批`,
        },
        ...mockAuditTrail,
      ];
    }

    return transaction;
  },

  async decideApproval(transactionId, decision, decisionNote) {
    const transaction = mockTransactions.find((item) => item.id === transactionId);
    if (!transaction) {
      throw new Error("流水不存在");
    }
    if (transaction.approvalState !== "pending") {
      throw new Error("流水不是待审批状态");
    }
    if (decision === "reject" && !decisionNote?.trim()) {
      throw new Error("请输入驳回原因");
    }

    transaction.approvalState = decision === "approve" ? "approved" : "rejected";
    mockApprovalQueue = mockApprovalQueue.filter((item) => item.transactionId !== transactionId);
    mockAuditTrail = [
      {
        id: crypto.randomUUID(),
        ledgerId: transaction.ledgerId,
        action: decision === "approve" ? "transaction_approved" : "transaction_rejected",
        actorName: "我",
        resourceId: transaction.id,
        createdAt: nowIso(),
        summary:
          decision === "approve"
            ? `批准${transaction.title}`
            : `驳回${transaction.title}，原因：${decisionNote?.trim()}`,
      },
      ...mockAuditTrail,
    ];

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
    approvalQueue: mapApprovalQueue(ledger.id, overview).sort(
      (a, b) => Date.parse(b.submittedAt) - Date.parse(a.submittedAt),
    ),
    auditTrail: overview.auditLogs
      .filter((item) => item.ledgerId === ledger.id)
      .map(mapAuditLog)
      .sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt)),
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
    role: normalizeRole(ledger.role),
    organizationName: ledger.kind === "organization_public" ? ledger.scopeLabel : undefined,
    balanceCents,
    pendingCount: transactions.filter((item) => item.approvalState === "submitted").length,
    auditUnreadCount: overview.auditLogs.filter((item) => item.ledgerId === ledger.id).length,
    lastSyncedAt: latestTransaction,
  };
}

async function fetchCloudStatus(): Promise<UserSession["cloudStatus"]> {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 2500);

  try {
    const response = await fetch(`${cloudBaseUrl}/ready`, {
      cache: "no-store",
      signal: controller.signal,
    });
    if (!response.ok) {
      return {
        state: "offline",
        label: "云端异常",
        detail: `${cloudBaseUrl} HTTP ${response.status}`,
      };
    }
    const ready = (await response.json()) as { serverId?: string; status?: string };
    if (ready.status !== "ready") {
      return {
        state: "offline",
        label: "云端未就绪",
        detail: `${cloudBaseUrl} status=${ready.status ?? "unknown"}`,
      };
    }
    const serverLabel = ready.serverId ? ` · ${ready.serverId.slice(0, 8)}` : "";

    return {
      state: "online",
      label: `云端在线${serverLabel}`,
      detail: `${cloudBaseUrl}/ready`,
    };
  } catch (error) {
    return {
      state: "offline",
      label: "云端离线",
      detail: error instanceof Error ? error.message : cloudBaseUrl,
    };
  } finally {
    window.clearTimeout(timeout);
  }
}

function normalizeRole(role: string): Ledger["role"] {
  if (
    role === "owner" ||
    role === "admin" ||
    role === "accountant" ||
    role === "approver" ||
    role === "member" ||
    role === "auditor" ||
    role === "viewer"
  ) {
    return role;
  }

  return "viewer";
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
  const ledger = overview.ledgers.find((item) => item.id === ledgerId);
  const role = normalizeRole(ledger?.role ?? "viewer");
  const canApproveLedger = role === "owner" || role === "admin" || role === "approver";

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
      submittedById: transaction.createdByUserId,
      submittedAt: transaction.occurredAt,
      state: "pending",
      canDecide: canApproveLedger && transaction.createdByUserId !== overview.currentUser.id,
    }));
}

function mapAuditLog(entry: AuditLogDto): AuditLogEntry {
  return {
    id: entry.id,
    ledgerId: entry.ledgerId,
    action: normalizeAuditAction(entry.action),
    actorName: entry.actorDisplayName || entry.actorUserId,
    resourceId: entry.resourceId,
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
