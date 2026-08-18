import type {
  AccountDto,
  AuditPeriodDto,
  AuditLogDto,
  CategoryDto,
  FinancialAnalysisDto,
  FinancialMemberDetailDto,
  FinancialMonthDetailDto,
  LedgerDto,
  LedgerOverview,
  TransactionDto,
  TransactionMonthDto,
} from "../api";
import { cloudBaseUrl } from "../config";
import { invoke } from "@tauri-apps/api/core";
import type {
  ApprovalQueueItem,
  AnalysisMonths,
  AuthSession,
  AuditLogEntry,
  AuditPeriod,
  Category,
  FinancialAccount,
  FinancialAnalysis,
  FinancialMemberDetail,
  FinancialMonthDetail,
  Ledger,
  LedgerDashboard,
  LoginDraft,
  NewTransactionDraft,
  Transaction,
  UpdateProfileDraft,
  UserSession,
} from "../types";

export interface CloudLedgerApi {
  getStoredSession(): AuthSession | undefined;
  login(input: LoginDraft): Promise<UserSession>;
  getLoginSecurity(): Promise<LoginSecurity>;
  updateProfile(input: UpdateProfileDraft): Promise<UserSession>;
  logout(): Promise<void>;
  getUserSession(): Promise<UserSession>;
  checkCloudStatus(): Promise<UserSession["cloudStatus"]>;
  listLedgers(): Promise<Ledger[]>;
  getLedgerDashboard(ledgerId: string, month?: string, day?: string): Promise<LedgerDashboard>;
  getAuditPeriod(
    ledgerId: string,
    granularity: AuditPeriod["granularity"],
    period: string,
  ): Promise<AuditPeriod>;
  getFinancialAnalysis(ledgerId: string, months: AnalysisMonths): Promise<FinancialAnalysis>;
  getFinancialMonthDetail(ledgerId: string, month: string): Promise<FinancialMonthDetail>;
  getFinancialMemberDetail(
    ledgerId: string,
    months: AnalysisMonths,
    memberId: string,
  ): Promise<FinancialMemberDetail>;
  createCategory(
    ledgerId: string,
    name: string,
    direction: Category["direction"],
  ): Promise<Category>;
  createTransaction(draft: NewTransactionDraft, clientMutationId?: string): Promise<Transaction>;
  decideApproval(
    transactionId: string,
    decision: "approve" | "reject",
    decisionNote?: string,
  ): Promise<Transaction>;
  markTransactionPaid(transactionId: string): Promise<Transaction>;
  confirmTransactionReceipt(transactionId: string): Promise<Transaction>;
  listApprovalQueue(ledgerId: string): Promise<ApprovalQueueItem[]>;
  listAuditTrail(ledgerId: string): Promise<AuditLogEntry[]>;
  isAuthRequired(error: unknown): boolean;
  isTurnstileRequired(error: unknown): boolean;
}

export interface LoginSecurity {
  turnstileEnabled: boolean;
  turnstileSiteKey?: string;
}

let overviewCache: LedgerOverview | undefined;
const installationStorageKey = "cloudledger.installationId";
const isTauriRuntime =
  "__TAURI_INTERNALS__" in window || window.location.hostname === "tauri.localhost";
const isNgrokTunnel = (() => {
  try {
    const hostname = new URL(cloudBaseUrl).hostname;
    return (
      hostname.endsWith(".ngrok-free.dev") ||
      hostname.endsWith(".ngrok.app") ||
      hostname.endsWith(".ngrok.io")
    );
  } catch {
    return false;
  }
})();
let ngrokWarningBypass: Promise<boolean> | undefined;
let memorySession: AuthSession | undefined;
let sessionRestoreAttempted = false;

class AuthRequiredError extends Error {
  constructor(message = "请先登录") {
    super(message);
    this.name = "AuthRequiredError";
  }
}

class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
  ) {
    super(code || `HTTP ${status}`);
    this.name = "HttpError";
  }
}

const getOverview = async () => {
  overviewCache = await authenticatedJson<LedgerOverview>("/app/overview");
  return overviewCache;
};

const serverApi: CloudLedgerApi = {
  getStoredSession() {
    return memorySession;
  },

  async login(input) {
    const session = await authJson(
      isTauriRuntime ? "/auth/tauri/login" : "/auth/web/login",
      authPayload(input),
    );
    await storeSession(session);
    overviewCache = undefined;
    return {
      currentUser: session.user,
      cloudStatus: await fetchCloudStatus(),
    };
  },

  async getLoginSecurity() {
    const response = await cloudFetch("/auth/security");
    if (!response.ok) throw await responseError(response);
    return response.json() as Promise<LoginSecurity>;
  },

  async updateProfile(input) {
    const me = await authenticatedJson<{ user: AuthSession["user"]; installationId: string }>(
      "/auth/me",
      {
        method: "PATCH",
        body: { displayName: input.displayName.trim() },
      },
    );
    const session = memorySession;
    if (session) {
      memorySession = normalizeSession({
        ...session,
        user: me.user,
        installationId: me.installationId,
      });
    }
    overviewCache = undefined;
    return {
      currentUser: me.user,
      cloudStatus: await fetchCloudStatus(),
    };
  },

  async logout() {
    try {
      if (isTauriRuntime) {
        await authenticatedJson("/auth/tauri/logout", { method: "POST", empty: true });
      } else {
        const response = await cloudFetch("/auth/web/logout", { method: "POST" });
        if (!response.ok && response.status !== 401) throw await responseError(response);
      }
    } catch (error) {
      if (!serverApi.isAuthRequired(error)) {
        throw error;
      }
    } finally {
      await clearSession();
      overviewCache = undefined;
    }
  },

  async getUserSession() {
    const session = await restoreSession();
    if (!session) {
      throw new AuthRequiredError();
    }
    const me = await authenticatedJson<{ user: AuthSession["user"]; installationId: string }>(
      isTauriRuntime ? "/auth/tauri/me" : "/auth/me",
    );
    return {
      currentUser: me.user,
      cloudStatus: await fetchCloudStatus(),
    };
  },

  async checkCloudStatus() {
    return fetchCloudStatus();
  },

  async listLedgers() {
    const overview = await getOverview();
    return overview.ledgers.map((ledger) => mapLedger(ledger, overview));
  },

  async getLedgerDashboard(ledgerId, month, day) {
    const overview = await getOverview();
    const monthQuery = month ? `&month=${encodeURIComponent(month)}` : "";
    const dayQuery = day ? `&day=${encodeURIComponent(day)}` : "";
    const transactionMonth = await authenticatedJson<TransactionMonthDto>(
      `/app/transactions?ledgerId=${encodeURIComponent(ledgerId)}${monthQuery}${dayQuery}`,
    );
    return mapDashboard(ledgerId, overview, transactionMonth);
  },

  async getAuditPeriod(ledgerId, granularity, period) {
    const audit = await authenticatedJson<AuditPeriodDto>(
      `/app/audit-period?ledgerId=${encodeURIComponent(ledgerId)}&granularity=${granularity}&period=${encodeURIComponent(period)}`,
    );
    return mapAuditPeriod(audit);
  },

  async getFinancialAnalysis(ledgerId, months) {
    const analysis = await authenticatedJson<FinancialAnalysisDto>(
      `/app/analytics?ledgerId=${encodeURIComponent(ledgerId)}&months=${months}`,
    );
    return mapFinancialAnalysis(analysis);
  },

  async getFinancialMonthDetail(ledgerId, month) {
    const detail = await authenticatedJson<FinancialMonthDetailDto>(
      `/app/analytics/month-detail?ledgerId=${encodeURIComponent(ledgerId)}&month=${encodeURIComponent(month)}`,
    );
    return mapFinancialMonthDetail(detail);
  },

  async getFinancialMemberDetail(ledgerId, months, memberId) {
    const detail = await authenticatedJson<FinancialMemberDetailDto>(
      `/app/analytics/member-detail?ledgerId=${encodeURIComponent(ledgerId)}&months=${months}&memberId=${encodeURIComponent(memberId)}`,
    );
    return mapFinancialMemberDetail(detail);
  },

  async createCategory(ledgerId, name, direction) {
    const category = await authenticatedJson<CategoryDto>("/app/categories", {
      method: "POST",
      body: { ledgerId, name, kind: direction },
    });
    overviewCache = undefined;
    return mapCategory(category);
  },

  async createTransaction(draft, clientMutationId) {
    const overview = overviewCache ?? (await getOverview());
    const account = overview.accounts.find((item) => item.id === draft.accountId);
    const category = categoryFromDraft(draft, overview);
    const ledger = overview.ledgers.find((item) => item.id === draft.ledgerId);
    const created = await authenticatedJson<TransactionDto>("/app/transactions", {
      method: "POST",
      body: {
        ledgerId: draft.ledgerId,
        accountId: draft.accountId,
        categoryId: draft.categoryId,
        kind: draft.direction,
        amountMinor: draft.amountCents,
        currency: account?.currency ?? "CNY",
        description: draft.memo?.trim() || category?.name || ledger?.name || "未命名流水",
        clientMutationId,
      },
    });

    overviewCache = undefined;
    return mapTransaction(created, overview);
  },

  async decideApproval(transactionId, decision, decisionNote) {
    const overview = overviewCache ?? (await getOverview());
    const decided = await authenticatedJson<TransactionDto>("/app/approvals/decide", {
      method: "POST",
      body: { transactionId, decision, decisionNote },
    });
    overviewCache = undefined;
    return mapTransaction(decided, overview);
  },

  async markTransactionPaid(transactionId) {
    const overview = overviewCache ?? (await getOverview());
    const transaction = await authenticatedJson<TransactionDto>("/app/payments/mark-paid", {
      method: "POST",
      body: { transactionId },
    });
    overviewCache = undefined;
    return mapTransaction(transaction, overview);
  },

  async confirmTransactionReceipt(transactionId) {
    const overview = overviewCache ?? (await getOverview());
    const transaction = await authenticatedJson<TransactionDto>("/app/payments/confirm-receipt", {
      method: "POST",
      body: { transactionId },
    });
    overviewCache = undefined;
    return mapTransaction(transaction, overview);
  },

  async listApprovalQueue(ledgerId) {
    const overview = await getOverview();
    return mapApprovalQueue(ledgerId, overview);
  },

  async listAuditTrail(ledgerId) {
    const overview = await getOverview();
    return overview.auditLogs.filter((item) => item.ledgerId === ledgerId).map(mapAuditLog);
  },

  isAuthRequired(error) {
    return error instanceof AuthRequiredError;
  },

  isTurnstileRequired(error) {
    return error instanceof HttpError && error.status === 428 && error.code === "turnstile_required";
  },
};

interface JsonRequestOptions {
  method?: "GET" | "POST" | "PATCH";
  body?: unknown;
  empty?: boolean;
}

function authPayload(input: LoginDraft) {
  const identifier = input.identifier.trim();
  const isEmail = identifier.includes("@");

  return {
    email: isEmail ? identifier : undefined,
    phone: isEmail ? undefined : identifier,
    password: input.password,
    installationId: getInstallationId(),
    turnstileToken: input.turnstileToken,
  };
}

async function authJson(path: string, body: unknown): Promise<AuthSession> {
  const response = await cloudFetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw await responseError(response);
  }
  return normalizeSession((await response.json()) as AuthSession);
}

async function authenticatedJson<T>(
  path: string,
  options: JsonRequestOptions = {},
  allowRefresh = true,
): Promise<T> {
  const session = memorySession;
  if (!session) {
    throw new AuthRequiredError();
  }

  const response = await cloudFetch(path, {
    method: options.method ?? "GET",
    headers: {
      Authorization: `Bearer ${session.accessToken}`,
      "Content-Type": "application/json",
    },
    body: options.body ? JSON.stringify(options.body) : undefined,
  });

  if (response.status === 401 && allowRefresh) {
      await refreshStoredSession(session);
    return authenticatedJson<T>(path, options, false);
  }

  if (!response.ok) {
    if (response.status === 401) {
      await clearSession();
      throw new AuthRequiredError();
    }
    throw await responseError(response);
  }

  if (options.empty || response.status === 204) {
    return undefined as T;
  }

  return response.json() as Promise<T>;
}

async function refreshStoredSession(session: AuthSession) {
  const path = isTauriRuntime ? "/auth/tauri/refresh" : "/auth/web/refresh";
  const response = await cloudFetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "include",
    body: isTauriRuntime
      ? JSON.stringify({
          refreshToken: session.refreshToken,
          installationId: session.installationId,
        })
      : JSON.stringify({ installationId: session.installationId }),
  });

  if (!response.ok) {
    if (response.status === 400 || response.status === 401 || response.status === 403) {
      await clearSession();
      throw new AuthRequiredError();
    }
    throw await responseError(response);
  }

  await storeSession(normalizeSession((await response.json()) as AuthSession));
}

async function responseError(response: Response) {
  const body = (await response.json().catch(() => ({}))) as { error?: string };
  return new HttpError(response.status, body.error ?? "");
}

async function cloudFetch(path: string, init?: RequestInit) {
  const controller = init?.signal ? undefined : new AbortController();
  const timeout = controller ? window.setTimeout(() => controller.abort(), 5_000) : undefined;
  try {
    const headers = new Headers(init?.headers);
    if (await ngrokWarningBypassEnabled()) {
      headers.set("ngrok-skip-browser-warning", "true");
    }
    return await fetch(`${cloudBaseUrl}${path}`, {
      credentials: isTauriRuntime ? init?.credentials : "include",
      ...init,
      headers,
      signal: init?.signal ?? controller?.signal,
    });
  } catch (error) {
    const detail = error instanceof DOMException && error.name === "AbortError" ? "请求超时" : "网络请求失败";
    throw new Error(`无法连接后端 ${cloudBaseUrl}（${detail}）`, { cause: error });
  } finally {
    if (timeout !== undefined) window.clearTimeout(timeout);
  }
}

async function ngrokWarningBypassEnabled() {
  if (!isNgrokTunnel) return false;
  const probe =
    ngrokWarningBypass ??
    fetch(`${cloudBaseUrl}/auth/bootstrap`, {
      method: "POST",
      credentials: isTauriRuntime ? undefined : "include",
    })
      .then(async (response) => {
        if (!response.ok) return false;
        const bootstrap = (await response.json()) as { ngrokWarningBypassEnabled?: boolean };
        return bootstrap.ngrokWarningBypassEnabled === true;
      })
      .catch(() => false);
  ngrokWarningBypass = probe;
  const enabled = await probe;
  if (!enabled && ngrokWarningBypass === probe) {
    ngrokWarningBypass = undefined;
  }
  return enabled;
}

async function restoreSession(): Promise<AuthSession | undefined> {
  if (memorySession || sessionRestoreAttempted) return memorySession;
  sessionRestoreAttempted = true;
  try {
    if (isTauriRuntime) {
      const secure = await invoke<{ refreshToken: string; installationId: string } | null>(
        "secure_session_load",
      );
      if (secure) {
        await refreshStoredSession({
          accessToken: "",
          refreshToken: secure.refreshToken,
          installationId: secure.installationId,
          user: { id: "", displayName: "" },
        });
      }
    } else {
      try {
        await refreshStoredSession({
          accessToken: "",
          installationId: getInstallationId(),
          user: { id: "", displayName: "" },
        });
      } catch (error) {
        if (!(error instanceof AuthRequiredError)) throw error;
      }
    }
  } catch (error) {
    if (!(error instanceof AuthRequiredError)) {
      sessionRestoreAttempted = false;
    }
    throw error;
  }
  return memorySession;
}

async function storeSession(session: AuthSession) {
  const normalized = normalizeSession(session);
  memorySession = normalized;
  sessionRestoreAttempted = true;
  if (isTauriRuntime && normalized.refreshToken) {
    await invoke("secure_session_store", {
      refreshToken: normalized.refreshToken,
      installationId: normalized.installationId,
    });
  }
}

async function clearSession() {
  memorySession = undefined;
  sessionRestoreAttempted = true;
  if (isTauriRuntime) {
    await invoke("secure_session_clear");
  }
}

function normalizeSession(session: AuthSession): AuthSession {
  if (!session.accessToken || !session.installationId || !session.user?.id) {
    throw new AuthRequiredError("登录状态无效");
  }

  return {
    ...session,
    user: {
      id: String(session.user.id),
      displayName: session.user.displayName,
      email: session.user.email,
      phone: session.user.phone,
    },
  };
}

function getInstallationId() {
  const existing = window.localStorage.getItem(installationStorageKey);
  if (existing) {
    return existing;
  }

  const created =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `install_${Date.now()}_${Math.random().toString(16).slice(2)}`;
  window.localStorage.setItem(installationStorageKey, created);
  return created;
}

const beijingDateFormatter = new Intl.DateTimeFormat("en-CA", {
  timeZone: "Asia/Shanghai",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});
const nowIso = () => new Date().toISOString();
const transactionDayKey = (value: string) => {
  const parts = beijingDateFormatter.formatToParts(new Date(value));
  const get = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((part) => part.type === type)?.value ?? "";
  return `${get("year")}-${get("month")}-${get("day")}`;
};
const transactionMonthKey = (value: string) => transactionDayKey(value).slice(0, 7);
const currentMonthKey = () => transactionMonthKey(nowIso());
const mockEmployeeMode = import.meta.env.VITE_CLOUDLEDGER_MOCK_ROLE === "employee";

const mockLedgers: Ledger[] = [
  {
    id: "personal-main",
    name: "个人账本",
    kind: "private",
    currency: "CNY",
    role: "owner",
    canViewBalances: true,
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
    role: mockEmployeeMode ? "employee" : "business_owner",
    canViewBalances: !mockEmployeeMode,
    organizationName: "CloudLedger Inc.",
    balanceCents: mockEmployeeMode ? null : 8429500,
    pendingCount: mockEmployeeMode ? 0 : 3,
    auditUnreadCount: mockEmployeeMode ? 0 : 8,
    lastSyncedAt: mockEmployeeMode ? undefined : nowIso(),
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
    id: "wechat",
    ledgerId: "personal-main",
    name: "微信",
    kind: "wechat",
    balanceCents: 152220,
  },
  {
    id: "alipay-personal",
    ledgerId: "personal-main",
    name: "支付宝",
    kind: "alipay",
    balanceCents: 0,
  },
  {
    id: "bank-personal",
    ledgerId: "personal-main",
    name: "银行账户",
    kind: "bank",
    balanceCents: 0,
  },
  {
    id: "wechat-org",
    ledgerId: "org-growth",
    name: "微信",
    kind: "wechat",
    balanceCents: mockEmployeeMode ? null : 0,
  },
  {
    id: "alipay-org",
    ledgerId: "org-growth",
    name: "支付宝",
    kind: "alipay",
    balanceCents: mockEmployeeMode ? null : 0,
  },
  {
    id: "company-bank",
    ledgerId: "org-growth",
    name: "银行账户",
    kind: "bank",
    balanceCents: mockEmployeeMode ? null : 8429500,
  },
  {
    id: "cash-org",
    ledgerId: "org-growth",
    name: "现金",
    kind: "cash",
    balanceCents: mockEmployeeMode ? null : 0,
  },
];

let mockCategories: Category[] = [
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
    accountName: "微信",
    categoryName: "餐饮",
    approvalState: "approved",
    paymentState: "not_applicable",
    actorName: "我",
    createdByUserId: "demo-user",
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
    accountName: "微信",
    categoryName: "交通",
    approvalState: "approved",
    paymentState: "not_applicable",
    actorName: "我",
    createdByUserId: "demo-user",
    auditRequired: false,
  },
  {
    id: "tx-201",
    ledgerId: "org-growth",
    occurredAt: "2026-07-08T02:20:00.000Z",
    title: "对象存储月账单",
    amountCents: 98600,
    direction: "expense",
    accountName: "银行账户",
    categoryName: "云资源",
    approvalState: "pending",
    paymentState: "not_applicable",
    actorName: "林会计",
    createdByUserId: "demo-lin",
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
    accountName: "银行账户",
    categoryName: "合同回款",
    approvalState: "approved",
    paymentState: "not_applicable",
    actorName: "陈经理",
    createdByUserId: "demo-chen",
    auditRequired: true,
  },
  {
    id: "tx-204",
    ledgerId: "org-growth",
    occurredAt: "2026-07-07T09:30:00.000Z",
    title: "客户拜访交通",
    amountCents: 42680,
    direction: "expense",
    accountName: "银行账户",
    categoryName: "差旅",
    approvalState: "approved",
    paymentState: "paid_pending_receipt",
    actorName: "周运营",
    createdByUserId: "demo-zhou",
    paidAt: "2026-07-08T03:15:00.000Z",
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
  {
    id: "audit-304",
    ledgerId: "org-growth",
    action: "transaction_paid",
    actorName: "陈经理",
    resourceId: "tx-204",
    createdAt: "2026-07-08T03:15:00.000Z",
    summary: "已打款客户拜访交通",
  },
];

const mockApi: CloudLedgerApi = {
  getStoredSession() {
    return {
      user: { id: "demo-user", displayName: "Alice" },
      accessToken: "mock-access",
      refreshToken: "mock-refresh",
      installationId: "mock-installation",
    };
  },

  async login(input) {
    return {
      currentUser: { id: "demo-user", displayName: input.identifier.trim() || "Alice" },
      cloudStatus: mockCloudStatus(),
    };
  },

  async getLoginSecurity() {
    return { turnstileEnabled: false };
  },

  async updateProfile(input) {
    return {
      currentUser: { id: "demo-user", displayName: input.displayName.trim() || "Alice" },
      cloudStatus: mockCloudStatus(),
    };
  },

  async logout() {
    return undefined;
  },

  async getUserSession() {
    return {
      currentUser: { id: "demo-user", displayName: "Alice" },
      cloudStatus: mockCloudStatus(),
    };
  },

  async checkCloudStatus() {
    return mockCloudStatus();
  },

  async listLedgers() {
    return mockLedgers;
  },

  async getLedgerDashboard(ledgerId, month, day) {
    const ledger = mockLedgers.find((item) => item.id === ledgerId) ?? mockLedgers[0];
    const selectedMonth = month ?? currentMonthKey();
    const visibleTransactions = mockTransactions.filter(
      (item) =>
        item.ledgerId === ledger.id &&
        (ledger.kind !== "organization" ||
          ledger.role === "business_owner" ||
          item.createdByUserId === "demo-user"),
    );
    const visibleTransactionIds = new Set(visibleTransactions.map((item) => item.id));
    const availableTransactionMonths = Array.from(
      new Set(visibleTransactions.map((item) => transactionMonthKey(item.occurredAt))),
    ).sort((left, right) => right.localeCompare(left));
    const availableTransactionDays = Array.from(
      new Set(
        visibleTransactions
          .filter((item) => transactionMonthKey(item.occurredAt) === selectedMonth)
          .map((item) => transactionDayKey(item.occurredAt)),
      ),
    ).sort((left, right) => right.localeCompare(left));
    return {
      ledger,
      accounts: mockAccounts.filter((item) => item.ledgerId === ledger.id),
      categories: mockCategories.filter((item) => item.ledgerId === ledger.id),
      selectedTransactionMonth: selectedMonth,
      availableTransactionMonths,
      selectedTransactionDay: day,
      availableTransactionDays,
      recentTransactions: visibleTransactions
        .filter(
          (item) =>
            transactionMonthKey(item.occurredAt) === selectedMonth &&
            (!day || transactionDayKey(item.occurredAt) === day),
        )
        .sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt)),
      approvalQueue: mockApprovalQueue.filter(
        (item) =>
          item.ledgerId === ledger.id &&
          (ledger.role === "business_owner" || item.submittedById === "demo-user"),
      ),
      auditTrail: mockAuditTrail.filter(
        (item) => item.ledgerId === ledger.id && visibleTransactionIds.has(item.resourceId),
      ),
    };
  },

  async getAuditPeriod(ledgerId, granularity, period) {
    const ledger = mockLedgers.find((item) => item.id === ledgerId) ?? mockLedgers[0];
    const visibleTransactionIds = new Set(
      mockTransactions
        .filter(
          (item) =>
            item.ledgerId === ledger.id &&
            (ledger.kind !== "organization" ||
              ledger.role === "business_owner" ||
              item.createdByUserId === "demo-user"),
        )
        .map((item) => item.id),
    );
    const grouped = new Map<string, AuditLogEntry[]>();
    for (const entry of mockAuditTrail) {
      if (entry.ledgerId !== ledger.id || !visibleTransactionIds.has(entry.resourceId)) continue;
      const steps = grouped.get(entry.resourceId) ?? [];
      steps.push(entry);
      grouped.set(entry.resourceId, steps);
    }
    const lifecycles = Array.from(grouped.entries())
      .map(([transactionId, steps]) => {
        const transaction = mockTransactions.find((item) => item.id === transactionId);
        if (!transaction) return undefined;
        const orderedSteps = [...steps].sort(
          (left, right) => Date.parse(left.createdAt) - Date.parse(right.createdAt),
        );
        const latestAt = orderedSteps[orderedSteps.length - 1]?.createdAt;
        if (!latestAt) return undefined;
        const latestPeriod = granularity === "day" ? transactionDayKey(latestAt) : transactionMonthKey(latestAt);
        if (latestPeriod !== period) return undefined;
        return {
          transactionId,
          description: transaction.title,
          direction: transaction.direction,
          amountCents: transaction.amountCents,
          currency: ledger.currency,
          occurredAt: transaction.occurredAt,
          approvalState: transaction.approvalState,
          paymentState: transaction.paymentState,
          latestAt,
          steps: orderedSteps,
        };
      })
      .filter((item): item is NonNullable<typeof item> => Boolean(item))
      .sort((left, right) => Date.parse(right.latestAt) - Date.parse(left.latestAt));
    const availableMonths = Array.from(
      new Set(
        Array.from(grouped.values()).map((steps) =>
          transactionMonthKey(steps.reduce((latest, item) => (item.createdAt > latest.createdAt ? item : latest)).createdAt),
        ),
      ),
    ).sort((left, right) => right.localeCompare(left));
    const availableDays = Array.from(
      new Set(
        Array.from(grouped.values()).map((steps) =>
          transactionDayKey(steps.reduce((latest, item) => (item.createdAt > latest.createdAt ? item : latest)).createdAt),
        ),
      ),
    ).sort((left, right) => right.localeCompare(left));
    return {
      ledgerId,
      granularity,
      period,
      availableMonths,
      availableDays,
      lifecycles,
    };
  },

  async getFinancialAnalysis(ledgerId, months) {
    return buildMockFinancialAnalysis(ledgerId, months);
  },

  async getFinancialMonthDetail(ledgerId, month) {
    return buildMockFinancialMonthDetail(ledgerId, month);
  },

  async getFinancialMemberDetail(ledgerId, months, memberId) {
    return buildMockFinancialMemberDetail(ledgerId, months, memberId);
  },

  async createCategory(ledgerId, name, direction) {
    const normalizedName = name.trim();
    if (
      mockCategories.some(
        (item) =>
          item.ledgerId === ledgerId &&
          item.direction === direction &&
          item.name.toLocaleLowerCase() === normalizedName.toLocaleLowerCase(),
      )
    ) {
      throw new Error("category already exists");
    }
    const category: Category = {
      id: crypto.randomUUID(),
      ledgerId,
      name: normalizedName,
      direction,
    };
    mockCategories = [...mockCategories, category];
    return category;
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
      paymentState: "not_applicable",
      actorName: "我",
      createdByUserId: "demo-user",
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
    transaction.paymentState =
      decision === "approve" && transaction.direction === "expense"
        ? "pending_payment"
        : "not_applicable";
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

  async markTransactionPaid(transactionId) {
    const transaction = mockTransactions.find((item) => item.id === transactionId);
    if (!transaction || transaction.paymentState !== "pending_payment") {
      throw new Error("流水不是待打款状态");
    }
    transaction.paymentState = "paid_pending_receipt";
    transaction.paidAt = nowIso();
    return transaction;
  },

  async confirmTransactionReceipt(transactionId) {
    const transaction = mockTransactions.find((item) => item.id === transactionId);
    if (!transaction || transaction.paymentState !== "paid_pending_receipt") {
      throw new Error("流水不是待确认收款状态");
    }
    transaction.paymentState = "received";
    transaction.receivedAt = nowIso();
    return transaction;
  },

  async listApprovalQueue(ledgerId) {
    const ledger = mockLedgers.find((item) => item.id === ledgerId);
    return mockApprovalQueue.filter(
      (item) =>
        item.ledgerId === ledgerId &&
        (ledger?.role === "business_owner" || item.submittedById === "demo-user"),
    );
  },

  async listAuditTrail(ledgerId) {
    const ledger = mockLedgers.find((item) => item.id === ledgerId);
    const visibleTransactionIds = new Set(
      mockTransactions
        .filter(
          (item) =>
            item.ledgerId === ledgerId &&
            (ledger?.role === "business_owner" || item.createdByUserId === "demo-user"),
        )
        .map((item) => item.id),
    );
    return mockAuditTrail.filter(
      (item) => item.ledgerId === ledgerId && visibleTransactionIds.has(item.resourceId),
    );
  },

  isAuthRequired() {
    return false;
  },

  isTurnstileRequired() {
    return false;
  },
};

export const cloudLedgerApi: CloudLedgerApi =
  import.meta.env.VITE_CLOUDLEDGER_USE_MOCK === "1" ? mockApi : serverApi;

function mapFinancialAnalysis(analysis: FinancialAnalysisDto): FinancialAnalysis {
  const months: AnalysisMonths =
    analysis.months === 3 || analysis.months === 12 ? analysis.months : 6;
  return {
    ledgerId: analysis.ledgerId,
    currency: analysis.currency,
    months,
    periodStart: analysis.periodStart,
    periodEnd: analysis.periodEnd,
    currentBalanceCents: analysis.currentBalanceMinor,
    incomeCents: analysis.incomeMinor,
    expenseCents: analysis.expenseMinor,
    netCashFlowCents: analysis.netCashFlowMinor,
    previousIncomeCents: analysis.previousIncomeMinor,
    previousExpenseCents: analysis.previousExpenseMinor,
    previousNetCashFlowCents: analysis.previousNetCashFlowMinor,
    transactionCount: analysis.transactionCount,
    pendingApproval: {
      count: analysis.pendingApproval.count,
      amountCents: analysis.pendingApproval.amountMinor,
    },
    pendingPayment: {
      count: analysis.pendingPayment.count,
      amountCents: analysis.pendingPayment.amountMinor,
    },
    paidPendingReceipt: {
      count: analysis.paidPendingReceipt.count,
      amountCents: analysis.paidPendingReceipt.amountMinor,
    },
    trend: analysis.trend.map((point) => ({
      key: point.key,
      label: point.label,
      incomeCents: point.incomeMinor,
      expenseCents: point.expenseMinor,
      netCashFlowCents: point.netCashFlowMinor,
    })),
    accounts: analysis.accounts.map((account) => ({
      id: account.id,
      name: account.name,
      kind: account.kind,
      balanceCents: account.balanceMinor,
    })),
    memberExpenses: analysis.memberExpenses.map((member) => ({
      userId: member.userId,
      displayName: member.displayName,
      expenseCents: member.expenseMinor,
      transactionCount: member.transactionCount,
    })),
    largestExpenses: analysis.largestExpenses.map((expense) => ({
      transactionId: expense.transactionId,
      description: expense.description,
      submittedBy: expense.submittedBy,
      amountCents: expense.amountMinor,
      paidAt: expense.paidAt,
    })),
    generatedAt: analysis.generatedAt,
  };
}

function mapFinancialMonthDetail(detail: FinancialMonthDetailDto): FinancialMonthDetail {
  return {
    ledgerId: detail.ledgerId,
    month: detail.month,
    currency: detail.currency,
    incomeCents: detail.incomeMinor,
    expenseCents: detail.expenseMinor,
    netCashFlowCents: detail.netCashFlowMinor,
    transactionCount: detail.transactionCount,
    categories: detail.categories.map((category) => ({
      categoryId: category.categoryId,
      categoryName: category.categoryName,
      direction: category.kind,
      amountCents: category.amountMinor,
      transactionCount: category.transactionCount,
    })),
    memberExpenses: detail.memberExpenses.map((member) => ({
      userId: member.userId,
      displayName: member.displayName,
      expenseCents: member.expenseMinor,
      transactionCount: member.transactionCount,
    })),
    transactions: detail.transactions.map(mapAnalysisTransaction),
  };
}

function mapFinancialMemberDetail(detail: FinancialMemberDetailDto): FinancialMemberDetail {
  const months: AnalysisMonths =
    detail.months === 3 || detail.months === 12 ? detail.months : 6;
  return {
    ledgerId: detail.ledgerId,
    currency: detail.currency,
    months,
    periodStart: detail.periodStart,
    periodEnd: detail.periodEnd,
    memberId: detail.memberId,
    displayName: detail.displayName,
    expenseCents: detail.expenseMinor,
    transactionCount: detail.transactionCount,
    transactions: detail.transactions.map(mapAnalysisTransaction),
  };
}

function mapAnalysisTransaction(
  transaction: FinancialMonthDetailDto["transactions"][number],
) {
  return {
    transactionId: transaction.transactionId,
    description: transaction.description,
    direction: transaction.kind,
    categoryId: transaction.categoryId,
    categoryName: transaction.categoryName,
    accountId: transaction.accountId,
    accountName: transaction.accountName,
    submittedByUserId: transaction.submittedByUserId,
    submittedBy: transaction.submittedBy,
    amountCents: transaction.amountMinor,
    effectiveAt: transaction.effectiveAt,
    paymentState: transaction.paymentState,
  };
}

const mockBusinessMembers = [
  { userId: "demo-user", displayName: "Alice" },
  { userId: "demo-lin", displayName: "林会计" },
  { userId: "demo-chen", displayName: "陈经理" },
  { userId: "demo-zhou", displayName: "周运营" },
  { userId: "demo-zero", displayName: "王新人" },
];

function mockCloudStatus(): UserSession["cloudStatus"] {
  return { state: "online", label: "Mock 数据" };
}

function mockActualCashFlows(ledgerId: string) {
  return mockTransactions
    .filter((transaction) => transaction.ledgerId === ledgerId)
    .flatMap((transaction) => {
      if (transaction.approvalState !== "approved") return [];
      if (transaction.direction === "expense" && transaction.paymentState === "pending_payment") {
        return [];
      }
      return [{
        transaction,
        effectiveAt: new Date(
          transaction.direction === "expense"
            ? transaction.paidAt || transaction.receivedAt || transaction.occurredAt
            : transaction.occurredAt,
        ),
      }];
    });
}

function buildMockFinancialMonthDetail(
  ledgerId: string,
  month: string,
): FinancialMonthDetail {
  const ledger = mockLedgers.find((item) => item.id === ledgerId);
  if (!ledger || ledger.kind !== "organization" || ledger.role !== "business_owner") {
    throw new Error("actor is not authorized for this action");
  }
  const flows = mockActualCashFlows(ledgerId).filter(
    (flow) => transactionMonthKey(flow.effectiveAt.toISOString()) === month,
  );
  const incomeCents = flows
    .filter((flow) => flow.transaction.direction === "income")
    .reduce((sum, flow) => sum + flow.transaction.amountCents, 0);
  const expenseCents = flows
    .filter((flow) => flow.transaction.direction === "expense")
    .reduce((sum, flow) => sum + flow.transaction.amountCents, 0);
  const categoryMap = new Map<string, FinancialMonthDetail["categories"][number]>();
  const memberMap = new Map(
    mockBusinessMembers.map((member) => [member.userId, { ...member, expenseCents: 0, transactionCount: 0 }]),
  );
  for (const flow of flows) {
    const transaction = flow.transaction;
    const key = `${transaction.direction}:${transaction.categoryName}`;
    const category = categoryMap.get(key) ?? {
      categoryName: transaction.categoryName,
      direction: transaction.direction,
      amountCents: 0,
      transactionCount: 0,
    };
    category.amountCents += transaction.amountCents;
    category.transactionCount += 1;
    categoryMap.set(key, category);
    if (transaction.direction === "expense") {
      const member = memberMap.get(transaction.createdByUserId) ?? {
        userId: transaction.createdByUserId,
        displayName: transaction.actorName,
        expenseCents: 0,
        transactionCount: 0,
      };
      member.expenseCents += transaction.amountCents;
      member.transactionCount += 1;
      memberMap.set(member.userId, member);
    }
  }
  return {
    ledgerId,
    month,
    currency: ledger.currency,
    incomeCents,
    expenseCents,
    netCashFlowCents: incomeCents - expenseCents,
    transactionCount: flows.length,
    categories: Array.from(categoryMap.values()).sort((a, b) => b.amountCents - a.amountCents),
    memberExpenses: Array.from(memberMap.values()).sort((a, b) => b.expenseCents - a.expenseCents),
    transactions: flows
      .sort((a, b) => b.effectiveAt.getTime() - a.effectiveAt.getTime())
      .map((flow) => ({
        transactionId: flow.transaction.id,
        description: flow.transaction.title,
        direction: flow.transaction.direction,
        categoryName: flow.transaction.categoryName,
        accountId: "mock-account",
        accountName: flow.transaction.accountName,
        submittedByUserId: flow.transaction.createdByUserId,
        submittedBy: flow.transaction.actorName,
        amountCents: flow.transaction.amountCents,
        effectiveAt: flow.effectiveAt.toISOString(),
        paymentState: flow.transaction.paymentState,
      })),
  };
}

function buildMockFinancialMemberDetail(
  ledgerId: string,
  months: AnalysisMonths,
  memberId: string,
): FinancialMemberDetail {
  const ledger = mockLedgers.find((item) => item.id === ledgerId);
  const member = mockBusinessMembers.find((item) => item.userId === memberId);
  if (!ledger || ledger.kind !== "organization" || ledger.role !== "business_owner" || !member) {
    throw new Error("actor is not authorized for this action");
  }
  const now = new Date();
  const periodStart = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - months + 1, 1));
  const transactions = mockActualCashFlows(ledgerId)
    .filter(
      (flow) =>
        flow.transaction.direction === "expense" &&
        flow.transaction.createdByUserId === memberId &&
        flow.effectiveAt >= periodStart &&
        flow.effectiveAt <= now,
    )
    .sort((a, b) => b.effectiveAt.getTime() - a.effectiveAt.getTime())
    .map((flow) => ({
      transactionId: flow.transaction.id,
      description: flow.transaction.title,
      direction: flow.transaction.direction,
      categoryName: flow.transaction.categoryName,
      accountId: "mock-account",
      accountName: flow.transaction.accountName,
      submittedByUserId: flow.transaction.createdByUserId,
      submittedBy: flow.transaction.actorName,
      amountCents: flow.transaction.amountCents,
      effectiveAt: flow.effectiveAt.toISOString(),
      paymentState: flow.transaction.paymentState,
    }));
  return {
    ledgerId,
    currency: ledger.currency,
    months,
    periodStart: periodStart.toISOString(),
    periodEnd: now.toISOString(),
    memberId,
    displayName: member.displayName,
    expenseCents: transactions.reduce((sum, transaction) => sum + transaction.amountCents, 0),
    transactionCount: transactions.length,
    transactions,
  };
}

function buildMockFinancialAnalysis(
  ledgerId: string,
  months: AnalysisMonths,
): FinancialAnalysis {
  const ledger = mockLedgers.find((item) => item.id === ledgerId);
  if (!ledger || ledger.kind !== "organization" || ledger.role !== "business_owner") {
    throw new Error("actor is not authorized for this action");
  }

  const now = new Date();
  const periodStart = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - months + 1, 1));
  const periodDuration = now.getTime() - periodStart.getTime();
  const previousStart = new Date(periodStart.getTime() - periodDuration);
  const cashFlows = mockActualCashFlows(ledgerId);
  const periodFlows = cashFlows.filter(
    (flow) => flow.effectiveAt >= periodStart && flow.effectiveAt <= now,
  );
  const previousFlows = cashFlows.filter(
    (flow) => flow.effectiveAt >= previousStart && flow.effectiveAt < periodStart,
  );
  const totals = (flows: typeof cashFlows) => {
    const incomeCents = flows
      .filter((flow) => flow.transaction.direction === "income")
      .reduce((sum, flow) => sum + flow.transaction.amountCents, 0);
    const expenseCents = flows
      .filter((flow) => flow.transaction.direction === "expense")
      .reduce((sum, flow) => sum + flow.transaction.amountCents, 0);
    return { incomeCents, expenseCents, netCashFlowCents: incomeCents - expenseCents };
  };
  const currentTotals = totals(periodFlows);
  const previousTotals = totals(previousFlows);
  const trend = Array.from({ length: months }, (_, index) => {
    const start = new Date(
      Date.UTC(periodStart.getUTCFullYear(), periodStart.getUTCMonth() + index, 1),
    );
    const end = new Date(Date.UTC(start.getUTCFullYear(), start.getUTCMonth() + 1, 1));
    const pointTotals = totals(
      periodFlows.filter((flow) => flow.effectiveAt >= start && flow.effectiveAt < end),
    );
    return {
      key: `${start.getUTCFullYear()}-${String(start.getUTCMonth() + 1).padStart(2, "0")}`,
      label: `${start.getUTCMonth() + 1}月`,
      ...pointTotals,
    };
  });
  const exposure = (state: Transaction["paymentState"] | "pending_approval") => {
    const transactions = mockTransactions.filter((transaction) => {
      if (transaction.ledgerId !== ledgerId || transaction.direction !== "expense") return false;
      return state === "pending_approval"
        ? transaction.approvalState === "pending"
        : transaction.approvalState === "approved" && transaction.paymentState === state;
    });
    return {
      count: transactions.length,
      amountCents: transactions.reduce((sum, transaction) => sum + transaction.amountCents, 0),
    };
  };
  const memberMap = new Map<string, { displayName: string; expenseCents: number; transactionCount: number }>(
    mockBusinessMembers.map((member) => [
      member.userId,
      { displayName: member.displayName, expenseCents: 0, transactionCount: 0 },
    ]),
  );
  for (const flow of periodFlows.filter((item) => item.transaction.direction === "expense")) {
    const transaction = flow.transaction;
    const current = memberMap.get(transaction.createdByUserId) ?? {
      displayName: transaction.actorName,
      expenseCents: 0,
      transactionCount: 0,
    };
    current.expenseCents += transaction.amountCents;
    current.transactionCount += 1;
    memberMap.set(transaction.createdByUserId, current);
  }

  return {
    ledgerId,
    currency: ledger.currency,
    months,
    periodStart: periodStart.toISOString(),
    periodEnd: now.toISOString(),
    currentBalanceCents: mockAccounts
      .filter((account) => account.ledgerId === ledgerId)
      .reduce((sum, account) => sum + (account.balanceCents ?? 0), 0),
    ...currentTotals,
    previousIncomeCents: previousTotals.incomeCents,
    previousExpenseCents: previousTotals.expenseCents,
    previousNetCashFlowCents: previousTotals.netCashFlowCents,
    transactionCount: periodFlows.length,
    pendingApproval: exposure("pending_approval"),
    pendingPayment: exposure("pending_payment"),
    paidPendingReceipt: exposure("paid_pending_receipt"),
    trend,
    accounts: mockAccounts
      .filter((account) => account.ledgerId === ledgerId)
      .map((account) => ({
        id: account.id,
        name: account.name,
        kind: account.kind,
        balanceCents: account.balanceCents ?? 0,
      })),
    memberExpenses: Array.from(memberMap, ([userId, value]) => ({ userId, ...value })).sort(
      (left, right) => right.expenseCents - left.expenseCents,
    ),
    largestExpenses: periodFlows
      .filter((flow) => flow.transaction.direction === "expense")
      .sort((left, right) => right.transaction.amountCents - left.transaction.amountCents)
      .slice(0, 5)
      .map((flow) => ({
        transactionId: flow.transaction.id,
        description: flow.transaction.title,
        submittedBy: flow.transaction.actorName,
        amountCents: flow.transaction.amountCents,
        paidAt: flow.effectiveAt.toISOString(),
      })),
    generatedAt: now.toISOString(),
  };
}

function mapDashboard(
  ledgerId: string,
  overview: LedgerOverview,
  transactionMonth: TransactionMonthDto,
): LedgerDashboard {
  const ledgerDto = overview.ledgers.find((item) => item.id === ledgerId) ?? overview.ledgers[0];
  const ledger = mapLedger(ledgerDto, overview);
  const accounts = overview.accounts.filter((item) => item.ledgerId === ledger.id).map(mapAccount);
  const categories = overview.categories
    .filter((item) => item.ledgerId === ledger.id)
    .map(mapCategory)
    .sort((left, right) => left.name.localeCompare(right.name, "zh-CN"));
  const recentTransactions = transactionMonth.transactions
    .map((item) => mapTransaction(item, overview))
    .sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt));

  return {
    ledger,
    accounts,
    categories,
    selectedTransactionMonth: transactionMonth.month,
    selectedTransactionDay: transactionMonth.day,
    availableTransactionMonths: transactionMonth.availableMonths,
    availableTransactionDays: transactionMonth.availableDays,
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
  const balanceCents = ledger.canViewBalances
    ? accounts.reduce((total, account) => total + (account.balanceMinor ?? 0), 0)
    : null;
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
    canViewBalances: ledger.canViewBalances,
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
    const response = await cloudFetch("/ready", {
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
    role === "business_owner" ||
    role === "employee"
  ) {
    return role;
  }

  return "employee";
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

function mapCategory(category: CategoryDto): Category {
  return {
    id: category.id,
    ledgerId: category.ledgerId,
    name: category.name,
    direction: category.kind,
  };
}

function mapTransaction(transaction: TransactionDto, overview: LedgerOverview): Transaction {
  const account = overview.accounts.find((item) => item.id === transaction.accountId);
  const category = overview.categories.find((item) => item.id === transaction.categoryId);

  return {
    id: transaction.id,
    ledgerId: transaction.ledgerId,
    occurredAt: transaction.occurredAt,
    title: transaction.description,
    amountCents: transaction.amountMinor,
    direction: transaction.kind === "income" ? "income" : "expense",
    accountName: account?.name ?? "未知账户",
    categoryName: category?.name ?? (transaction.kind === "income" ? "其他收入" : "其他支出"),
    approvalState: normalizeApprovalState(transaction.approvalState),
    paymentState: transaction.paymentState,
    actorName: transaction.createdBy,
    createdByUserId: transaction.createdByUserId,
    paidAt: transaction.paidAt,
    receivedAt: transaction.receivedAt,
    memo: transaction.description,
    auditRequired: transaction.approvalState === "submitted",
  };
}

function mapApprovalQueue(ledgerId: string, overview: LedgerOverview): ApprovalQueueItem[] {
  const ledger = overview.ledgers.find((item) => item.id === ledgerId);
  const role = normalizeRole(ledger?.role ?? "employee");
  const canApproveLedger = role === "business_owner";

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

function mapAuditPeriod(audit: AuditPeriodDto): AuditPeriod {
  return {
    ledgerId: audit.ledgerId,
    granularity: audit.granularity,
    period: audit.period,
    availableMonths: audit.availableMonths,
    availableDays: audit.availableDays,
    lifecycles: audit.lifecycles.map((lifecycle) => ({
      transactionId: lifecycle.transactionId,
      description: lifecycle.description,
      direction: lifecycle.kind === "income" ? "income" : "expense",
      amountCents: lifecycle.amountMinor,
      currency: lifecycle.currency,
      occurredAt: lifecycle.occurredAt,
      approvalState: normalizeApprovalState(lifecycle.approvalState),
      paymentState: lifecycle.paymentState,
      latestAt: lifecycle.latestAt,
      steps: lifecycle.steps.map(mapAuditLog),
    })),
  };
}

function categoryFromDraft(draft: NewTransactionDraft, overview: LedgerOverview): Category | undefined {
  return overview.categories
    .filter((category) => category.ledgerId === draft.ledgerId)
    .map(mapCategory)
    .find((category) => category.id === draft.categoryId);
}

function normalizeAccountKind(kind: string): FinancialAccount["kind"] {
  if (
    kind === "cash" ||
    kind === "bank" ||
    kind === "wechat" ||
    kind === "alipay" ||
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
  if (action.includes("auto_approved")) {
    return "transaction_auto_approved";
  }

  if (action.includes("received")) {
    return "transaction_received";
  }

  if (action.includes("paid")) {
    return "transaction_paid";
  }

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
