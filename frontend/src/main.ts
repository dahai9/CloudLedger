import { cloudLedgerApi } from "./api/cloudLedger";
import "./styles.css";
import type {
  AnalysisMonths,
  ApprovalState,
  Category,
  Ledger,
  LedgerDashboard,
  FinancialAnalysis,
  LoginDraft,
  NewTransactionDraft,
  TransactionDirection,
  UpdateProfileDraft,
  UserAccount,
  UserSession,
} from "./types";

type ViewMode = "activity" | "analysis" | "approval" | "audit";
type TransactionFilter = "all" | "pending" | "approved" | "rejected";
type AuthStatus = "checking" | "authenticated" | "anonymous";

interface QuickEntryForm {
  direction: TransactionDirection;
  amount: string;
  accountId: string;
  categoryId: string;
  memo: string;
  submitForApproval: boolean;
}

interface AuthForm {
  identifier: string;
  password: string;
  turnstileToken: string;
  turnstileSiteKey?: string;
}

interface ProfileForm {
  displayName: string;
}

interface AppState {
  authStatus: AuthStatus;
  userMenuOpen: boolean;
  profileEditing: boolean;
  ledgers: Ledger[];
  dashboard?: LedgerDashboard;
  analysis?: FinancialAnalysis;
  analysisMonths: AnalysisMonths;
  analysisLoading: boolean;
  analysisError?: string;
  activeLedgerId?: string;
  currentUser?: UserAccount;
  cloudStatus: UserSession["cloudStatus"];
  loading: boolean;
  pendingAction?: string;
  error?: string;
  view: ViewMode;
  filter: TransactionFilter;
  activityMonth: string;
  categoryEditing: boolean;
  categoryName: string;
  form: QuickEntryForm;
  authForm: AuthForm;
  profileForm: ProfileForm;
  toast?: string;
}

const appRoot = document.querySelector<HTMLDivElement>("#app");

if (!appRoot) {
  throw new Error("Missing #app root");
}

const app = appRoot;

const state: AppState = {
  authStatus: "checking",
  userMenuOpen: false,
  profileEditing: false,
  ledgers: [],
  cloudStatus: {
    state: "checking",
    label: "云端检测中",
  },
  loading: true,
  analysisMonths: 6,
  analysisLoading: false,
  view: "activity",
  filter: "all",
  activityMonth: currentMonthKey(),
  categoryEditing: false,
  categoryName: "",
  form: {
    direction: "expense",
    amount: "",
    accountId: "",
    categoryId: "",
    memo: "",
    submitForApproval: false,
  },
  authForm: {
    identifier: "",
    password: "",
    turnstileToken: "",
  },
  profileForm: {
    displayName: "",
  },
};

const moneyFormatterCache = new Map<string, Intl.NumberFormat>();
const dateFormatter = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});
const periodMonthFormatter = new Intl.DateTimeFormat("zh-CN", {
  year: "numeric",
  month: "short",
});
const autoRefreshMs = 10_000;
let autoRefreshInFlight = false;
let turnstileScriptPromise: Promise<void> | undefined;

void loadInitialState();
window.setInterval(() => {
  if (shouldAutoRefresh()) {
    void refreshRemoteState({ silent: true });
  }
}, autoRefreshMs);

window.addEventListener("focus", () => {
  if (state.authStatus === "authenticated") {
    void refreshRemoteState({ silent: true });
  }
});

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && state.authStatus === "authenticated") {
    void refreshRemoteState({ silent: true });
  }
});

document.addEventListener("click", (event) => {
  if (!state.userMenuOpen) {
    return;
  }
  const target = event.target instanceof Element ? event.target : undefined;
  if (target?.closest("#userMenuButton")) {
    return;
  }
  if (!target?.closest(".user-menu")) {
    closeUserMenu();
  }
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && state.userMenuOpen) {
    closeUserMenu();
    app.querySelector<HTMLButtonElement>("#userMenuButton")?.focus();
  }
});

async function loadInitialState() {
  try {
    state.loading = true;
    render();
    const session = await cloudLedgerApi.getUserSession();
    const ledgers = await cloudLedgerApi.listLedgers();
    const activeLedgerId = pickDefaultLedgerId(ledgers);

    state.currentUser = session.currentUser;
    state.cloudStatus = session.cloudStatus;
    state.ledgers = ledgers;
    state.activeLedgerId = activeLedgerId;
    state.dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(activeLedgerId, state.activityMonth)
      : undefined;
    state.activityMonth = state.dashboard?.selectedTransactionMonth ?? currentMonthKey();
    state.authStatus = "authenticated";
    syncProfileFormFromUser();
    resetFormForDashboard();
    state.error = undefined;
  } catch (error) {
    if (cloudLedgerApi.isAuthRequired(error)) {
      state.authStatus = "anonymous";
      state.currentUser = undefined;
      state.ledgers = [];
      state.activeLedgerId = undefined;
      state.dashboard = undefined;
      state.analysis = undefined;
      state.userMenuOpen = false;
      state.profileEditing = false;
      state.error = undefined;
    } else {
      state.error = friendlyError(error, "加载失败");
    }
  } finally {
    state.loading = false;
    render();
  }
}

async function submitAuthForm() {
  if (state.pendingAction) {
    return;
  }

  const identifier = state.authForm.identifier.trim();
  const password = state.authForm.password;
  if (!identifier || !password) {
    showToast("请填写完整登录信息");
    return;
  }

  try {
    state.loading = true;
    state.pendingAction = "auth";
    render();
    const draft: LoginDraft = {
      identifier,
      password,
      turnstileToken: state.authForm.turnstileToken || undefined,
    };
    await cloudLedgerApi.login(draft);
    state.authForm.turnstileToken = "";
    state.authForm.turnstileSiteKey = undefined;
    await loadInitialState();
  } catch (error) {
    if (cloudLedgerApi.isTurnstileRequired(error)) {
      try {
        const security = await cloudLedgerApi.getLoginSecurity();
        if (security.turnstileEnabled && security.turnstileSiteKey) {
          state.authForm.turnstileToken = "";
          state.authForm.turnstileSiteKey = security.turnstileSiteKey;
          return;
        }
      } catch {
        showToast("人机验证服务暂时不可用");
        return;
      }
    }
    state.authForm.turnstileToken = "";
    showToast(friendlyError(error, "登录失败"));
  } finally {
    state.loading = false;
    state.pendingAction = undefined;
    render();
  }
}

async function logout() {
  try {
    state.loading = true;
    state.userMenuOpen = false;
    state.profileEditing = false;
    render();
    await cloudLedgerApi.logout();
  } catch (error) {
    showToast(friendlyError(error, "退出失败"));
  } finally {
    state.authStatus = "anonymous";
    state.currentUser = undefined;
    state.ledgers = [];
    state.activeLedgerId = undefined;
    state.dashboard = undefined;
    state.analysis = undefined;
    state.loading = false;
    render();
  }
}

async function switchLedger(ledgerId: string) {
  try {
    state.loading = true;
    state.activeLedgerId = ledgerId;
    state.activityMonth = currentMonthKey();
    state.categoryEditing = false;
    state.categoryName = "";
    render();
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(ledgerId, state.activityMonth);
    state.activityMonth = state.dashboard.selectedTransactionMonth;
    state.analysis = undefined;
    state.analysisError = undefined;
    state.view = "activity";
    state.filter = "all";
    resetFormForDashboard();
    state.error = undefined;
  } catch (error) {
    state.error = friendlyError(error, "切换账本失败");
  } finally {
    state.loading = false;
    render();
  }
}

async function loadActivityMonth(month: string) {
  const ledgerId = state.activeLedgerId;
  if (!ledgerId || month === state.activityMonth || state.pendingAction) {
    return;
  }
  try {
    state.pendingAction = "activity-month";
    state.activityMonth = month;
    render();
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(ledgerId, month);
    state.activityMonth = state.dashboard.selectedTransactionMonth;
    resetFormForDashboard({ preserveDraft: true });
    state.error = undefined;
  } catch (error) {
    showToast(friendlyError(error, "加载月份流水失败"));
  } finally {
    state.pendingAction = undefined;
    render();
  }
}

async function refreshDashboard() {
  await refreshRemoteState({ silent: false, allowPending: true });
}

async function changeView(view: ViewMode) {
  state.view = view;
  render();
  if (view === "analysis") {
    await loadFinancialAnalysis(false);
  }
}

async function loadFinancialAnalysis(force: boolean) {
  const dashboard = state.dashboard;
  if (
    !dashboard ||
    dashboard.ledger.role !== "business_owner" ||
    dashboard.ledger.kind !== "organization" ||
    state.analysisLoading
  ) {
    return;
  }
  if (
    !force &&
    state.analysis?.ledgerId === dashboard.ledger.id &&
    state.analysis.months === state.analysisMonths
  ) {
    return;
  }

  const ledgerId = dashboard.ledger.id;
  const months = state.analysisMonths;
  state.analysisLoading = true;
  state.analysisError = undefined;
  render();
  try {
    const analysis = await cloudLedgerApi.getFinancialAnalysis(ledgerId, months);
    if (state.activeLedgerId === ledgerId && state.analysisMonths === months) {
      state.analysis = analysis;
    }
  } catch (error) {
    if (state.activeLedgerId === ledgerId && state.analysisMonths === months) {
      state.analysisError = friendlyError(error, "加载财务分析失败");
    }
  } finally {
    state.analysisLoading = false;
    render();
  }
}

async function refreshRemoteState(
  options: { silent: boolean; allowPending?: boolean } = { silent: true },
) {
  if (!state.activeLedgerId || autoRefreshInFlight || (state.pendingAction && !options.allowPending)) {
    return;
  }

  autoRefreshInFlight = true;
  try {
    const previousLedgerId = state.activeLedgerId;
    const session = await cloudLedgerApi.getUserSession();
    const ledgers = await cloudLedgerApi.listLedgers();
    const activeLedgerId = ledgers.some((ledger) => ledger.id === previousLedgerId)
      ? previousLedgerId
      : pickDefaultLedgerId(ledgers);

    state.currentUser = session.currentUser;
    state.cloudStatus = session.cloudStatus;
    state.ledgers = ledgers;
    state.activeLedgerId = activeLedgerId;
    state.dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(activeLedgerId, state.activityMonth)
      : undefined;
    state.activityMonth = state.dashboard?.selectedTransactionMonth ?? currentMonthKey();
    state.error = undefined;
    syncProfileFormFromUser();
    resetFormForDashboard({ preserveDraft: true });
  } catch (error) {
    if (cloudLedgerApi.isAuthRequired(error)) {
      state.authStatus = "anonymous";
      state.currentUser = undefined;
      state.ledgers = [];
      state.activeLedgerId = undefined;
      state.dashboard = undefined;
      state.analysis = undefined;
      state.userMenuOpen = false;
      state.profileEditing = false;
      state.error = undefined;
    } else {
      state.cloudStatus = await cloudLedgerApi.checkCloudStatus();
      if (!options.silent) {
        state.error = friendlyError(error, "刷新失败");
      }
    }
  } finally {
    autoRefreshInFlight = false;
    render();
  }
}

function shouldAutoRefresh() {
  return (
    state.authStatus === "authenticated" &&
    document.visibilityState !== "hidden" &&
    !state.loading &&
    !state.pendingAction
  );
}

function resetFormForDashboard(options: { preserveDraft?: boolean } = {}) {
  const dashboard = state.dashboard;
  if (!dashboard) {
    return;
  }

  const entryAccounts = entryAccountsForDashboard(dashboard);
  const preservedAccount = entryAccounts.find((account) => account.id === state.form.accountId);
  const firstAccount = preservedAccount ?? entryAccounts[0];
  const categories = categoriesForDirection(state.form.direction);
  const preservedCategory = categories.find((category) => category.id === state.form.categoryId);

  state.form = {
    ...state.form,
    amount: options.preserveDraft ? state.form.amount : "",
    accountId: firstAccount?.id ?? "",
    categoryId: preservedCategory?.id ?? categories[0]?.id ?? "",
    memo: options.preserveDraft ? state.form.memo : "",
    submitForApproval:
      dashboard.ledger.kind === "organization"
        ? true
        : options.preserveDraft
          ? state.form.submitForApproval
          : false,
  };
}

function syncProfileFormFromUser() {
  if (!state.profileEditing) {
    state.profileForm.displayName = state.currentUser?.displayName ?? "";
  }
}

function categoriesForDirection(direction: TransactionDirection): Category[] {
  return state.dashboard?.categories.filter((category) => category.direction === direction) ?? [];
}

function entryAccountsForDashboard(dashboard: LedgerDashboard) {
  const rank: Record<string, number> = { wechat: 0, alipay: 1, bank: 2, cash: 3 };
  return dashboard.accounts
    .filter((account) => account.kind in rank)
    .sort((left, right) => rank[left.kind] - rank[right.kind]);
}

function render() {
  const dashboard = state.dashboard;
  const ledger = dashboard?.ledger;

  app.innerHTML = `
    <main
      class="app-shell"
      data-auth-state="${escapeHtml(state.authStatus)}"
      data-current-user-id="${escapeHtml(state.currentUser?.id ?? "")}"
      data-active-ledger-id="${escapeHtml(state.activeLedgerId ?? "")}"
      data-cloud-state="${escapeHtml(state.cloudStatus.state)}"
    >
      ${state.authStatus === "authenticated" ? renderTopBar() : ""}
      ${
        state.authStatus === "anonymous"
          ? renderLogin()
          : state.loading && !dashboard
          ? renderLoading()
          : state.error
            ? renderError()
            : dashboard && ledger
              ? renderDashboard(dashboard)
              : renderEmptyState()
      }
      ${dashboard && ledger ? renderBottomNav(dashboard) : ""}
      ${state.toast ? `<div class="toast" role="status">${escapeHtml(state.toast)}</div>` : ""}
    </main>
  `;

  bindEvents();
  if (state.authStatus === "anonymous" && state.authForm.turnstileSiteKey) {
    void mountTurnstile().catch(() => {
      turnstileScriptPromise = undefined;
      state.authForm.turnstileToken = "";
      state.authForm.turnstileSiteKey = undefined;
      showToast("人机验证服务暂时不可用");
    });
  }
}

function pickDefaultLedgerId(ledgers: Ledger[]) {
  return ledgers.find((ledger) => ledger.kind === "private")?.id ?? ledgers[0]?.id;
}

function renderTopBar() {
  const ledger = state.dashboard?.ledger;

  return `
    <header class="top-bar">
      <div class="brand-block">
        <span class="brand-mark" aria-hidden="true">CL</span>
        <div>
          <h1>CloudLedger</h1>
          <p>${ledger ? `${ledgerKindLabel(ledger.kind)} · ${roleLabel(ledger.role)}` : "移动账本"}</p>
        </div>
      </div>
      <div class="top-actions">
        <span class="cloud-chip ${state.cloudStatus.state}">${escapeHtml(state.cloudStatus.label)}</span>
        ${renderUserMenu()}
      </div>
    </header>
    <div class="ledger-picker" id="ledgerSelect" role="group" aria-label="账本切换">
      <span>账本</span>
      <div class="switcher-row">
        ${state.ledgers.map(renderLedgerSwitchButton).join("")}
      </div>
    </div>
  `;
}

function renderUserMenu() {
  const user = state.currentUser;
  const displayName = user?.displayName ?? "未登录";
  const contact = user?.email || user?.phone || "未绑定联系方式";
  const editing = state.profileEditing;

  return `
    <div class="user-menu ${state.userMenuOpen ? "is-open" : ""}">
      <button
        class="avatar-button"
        id="userMenuButton"
        type="button"
        aria-label="账号菜单"
        aria-expanded="${state.userMenuOpen}"
        aria-controls="userMenuPanel"
        aria-haspopup="true"
      >
        ${escapeHtml(userInitial(displayName))}
      </button>
      ${
        state.userMenuOpen
          ? `
        <div class="user-menu-panel" id="userMenuPanel">
          <div class="user-menu-account">
            <span class="section-kicker">Account</span>
            <strong>${escapeHtml(displayName)}</strong>
            <p>${escapeHtml(contact)}</p>
            <p>ID ${escapeHtml(user?.id.slice(0, 8) ?? "-")}</p>
          </div>
          ${
            editing
              ? `
            <form class="profile-form" id="profileForm">
              <label>
                <span>显示名</span>
                <input
                  id="profileDisplayName"
                  autocomplete="name"
                  value="${escapeHtml(state.profileForm.displayName)}"
                />
              </label>
              <div class="profile-actions">
                <button class="secondary-button" type="submit" ${state.pendingAction === "profile" ? "disabled" : ""}>
                  ${state.pendingAction === "profile" ? "保存中" : "保存"}
                </button>
                <button class="ghost-button" id="cancelProfileEdit" type="button">
                  取消
                </button>
              </div>
            </form>
          `
              : `
            <button class="ghost-button user-menu-action" id="editProfileButton" type="button">
              编辑基本信息
            </button>
          `
          }
          <button class="danger-button user-menu-logout" data-user-menu-action="logout" type="button" ${
            state.loading ? "disabled" : ""
          }>
            退出登录
          </button>
        </div>
      `
          : ""
      }
    </div>
  `;
}

function renderLogin() {
  return `
    <section class="login-panel" aria-label="账号登录">
      <div class="login-brand">
        <span class="brand-mark" aria-hidden="true">CL</span>
        <div>
          <h1>CloudLedger</h1>
          <p>${state.cloudStatus.detail ? escapeHtml(state.cloudStatus.detail) : "移动账本"}</p>
        </div>
      </div>

      <form id="authForm" class="auth-form">
        <label>
          <span>邮箱或手机号</span>
          <input id="authIdentifier" autocomplete="username" value="${escapeHtml(state.authForm.identifier)}" />
        </label>
        <label>
          <span>密码</span>
          <input
            id="authPassword"
            type="password"
            autocomplete="current-password"
            value="${escapeHtml(state.authForm.password)}"
          />
        </label>
        ${
          state.authForm.turnstileSiteKey
            ? '<div class="turnstile-slot" id="businessTurnstile"></div>'
            : ""
        }
        <button class="primary-button" type="submit" ${
          state.loading || (state.authForm.turnstileSiteKey && !state.authForm.turnstileToken)
            ? "disabled"
            : ""
        }>
          ${state.pendingAction === "auth" ? "处理中" : "登录"}
        </button>
      </form>
    </section>
  `;
}

async function mountTurnstile() {
  const target = app.querySelector<HTMLElement>("#businessTurnstile");
  const sitekey = state.authForm.turnstileSiteKey;
  if (!target || !sitekey) return;
  await loadTurnstileScript();
  if (!window.turnstile || !target.isConnected || target.childElementCount > 0) return;
  window.turnstile.render(target, {
    sitekey,
    action: "business-login",
    callback(token) {
      state.authForm.turnstileToken = token;
      app.querySelector<HTMLButtonElement>("#authForm button[type='submit']")?.removeAttribute(
        "disabled",
      );
    },
    "expired-callback"() {
      state.authForm.turnstileToken = "";
      app.querySelector<HTMLButtonElement>("#authForm button[type='submit']")?.setAttribute(
        "disabled",
        "",
      );
    },
    "error-callback"() {
      state.authForm.turnstileToken = "";
      app.querySelector<HTMLButtonElement>("#authForm button[type='submit']")?.setAttribute(
        "disabled",
        "",
      );
    },
  });
}

function loadTurnstileScript(): Promise<void> {
  if (window.turnstile) return Promise.resolve();
  if (turnstileScriptPromise) return turnstileScriptPromise;
  turnstileScriptPromise = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    script.async = true;
    script.defer = true;
    script.addEventListener("load", () => resolve(), { once: true });
    script.addEventListener("error", () => reject(new Error("turnstile script unavailable")), {
      once: true,
    });
    document.head.append(script);
  });
  return turnstileScriptPromise;
}

function renderLedgerSwitchButton(ledger: Ledger) {
  const active = ledger.id === state.activeLedgerId;

  return `
    <button
      class="switcher-button ${active ? "is-active" : ""}"
      type="button"
      data-ledger-id="${escapeHtml(ledger.id)}"
      aria-pressed="${active}"
    >
      ${escapeHtml(ledger.name)}
    </button>
  `;
}

function renderDashboard(dashboard: LedgerDashboard) {
  return `
    ${renderBalancePanel(dashboard)}
    ${state.view === "analysis" ? "" : renderQuickEntry(dashboard)}
    ${renderActiveView(dashboard)}
  `;
}

function renderBalancePanel(dashboard: LedgerDashboard) {
  const { ledger } = dashboard;
  const pendingPaymentCount = dashboard.recentTransactions.filter(
    (transaction) => transaction.paymentState === "pending_payment",
  ).length;
  const pendingReceiptCount = dashboard.recentTransactions.filter(
    (transaction) => transaction.paymentState === "paid_pending_receipt",
  ).length;
  const organization = ledger.organizationName
    ? `<span class="context-pill">${escapeHtml(ledger.organizationName)}</span>`
    : "";

  return `
    <section class="balance-panel" aria-label="账本概览">
      <div>
        <span class="section-kicker">${ledgerKindLabel(ledger.kind)}</span>
        <p class="balance-label">${escapeHtml(ledger.name)}</p>
        <strong class="balance-value">${formatMoney(ledger.balanceCents, ledger.currency)}</strong>
      </div>
      <div class="balance-meta">
        ${organization}
        <span class="context-pill">${ledger.pendingCount} 待审</span>
        <span class="context-pill">${pendingPaymentCount} 待打款</span>
        <span class="context-pill">${pendingReceiptCount} 待确认</span>
        <span class="context-pill">${ledger.auditUnreadCount} 审计</span>
      </div>
      <p class="sync-line">最近流水 ${formatDate(ledger.lastSyncedAt)}</p>
    </section>
  `;
}

function renderQuickEntry(dashboard: LedgerDashboard) {
  const form = state.form;
  const categories = categoriesForDirection(form.direction);
  const accountOptions = entryAccountsForDashboard(dashboard)
    .map(
      (account) => `
        <option value="${escapeHtml(account.id)}" ${form.accountId === account.id ? "selected" : ""}>
          ${escapeHtml(account.name)}
        </option>
      `,
    )
    .join("");
  const categoryOptions = categories
    .map(
      (category) => `
        <option value="${escapeHtml(category.id)}" ${form.categoryId === category.id ? "selected" : ""}>
          ${escapeHtml(category.name)}
        </option>
      `,
    )
    .join("");

  return `
    <form id="quickEntryForm" class="quick-entry">
      <div class="section-heading">
        <div>
          <span class="section-kicker">Quick Entry</span>
          <h2>快速记账</h2>
        </div>
        <button class="ghost-button" type="button" data-view-target="activity">
          <span aria-hidden="true">↺</span>
          流水
        </button>
      </div>

      <div class="segmented-control" role="group" aria-label="收支方向">
        ${renderDirectionButton("expense", "支出")}
        ${renderDirectionButton("income", "收入")}
      </div>

      <label class="amount-field">
        <span>金额</span>
        <input
          id="amountInput"
          inputmode="decimal"
          autocomplete="off"
          placeholder="0.00"
          value="${escapeHtml(form.amount)}"
        />
      </label>

      <div class="field-grid">
        <label>
          <span>账户</span>
          <select id="accountSelect">${accountOptions}</select>
        </label>
        <div class="form-field category-field">
          <div class="field-label-row">
            <span>分类</span>
            <button
              id="addCategoryButton"
              class="icon-button"
              type="button"
              title="添加分类"
              aria-label="添加分类"
              aria-expanded="${state.categoryEditing}"
            >+</button>
          </div>
          <select id="categorySelect">${categoryOptions}</select>
        </div>
      </div>

      ${
        state.categoryEditing
          ? `<div class="category-editor">
              <input
                id="categoryNameInput"
                maxlength="24"
                autocomplete="off"
                placeholder="新${form.direction === "expense" ? "支出" : "收入"}分类"
                value="${escapeHtml(state.categoryName)}"
              />
              <button id="saveCategoryButton" class="icon-button is-confirm" type="button" title="保存分类" aria-label="保存分类" ${state.pendingAction === "create-category" ? "disabled" : ""}>✓</button>
              <button id="cancelCategoryButton" class="icon-button" type="button" title="取消" aria-label="取消">×</button>
            </div>`
          : ""
      }

      <label class="memo-field">
        <span>备注</span>
        <input id="memoInput" autocomplete="off" value="${escapeHtml(form.memo)}" placeholder="例如：团队午餐" />
      </label>

      <label class="approval-toggle">
        <input
          id="approvalToggle"
          type="checkbox"
          ${dashboard.ledger.kind === "organization" ? "checked disabled" : ""}
          ${dashboard.ledger.kind !== "organization" && form.submitForApproval ? "checked" : ""}
        />
        <span>${dashboard.ledger.kind === "organization" ? "公账审批流程" : "提交审批"}</span>
      </label>

      <button class="primary-button" type="submit" ${state.loading ? "disabled" : ""}>
        <span aria-hidden="true">+</span>
        ${state.pendingAction === "create" ? "保存中" : "保存流水"}
      </button>
    </form>
  `;
}

function renderDirectionButton(direction: TransactionDirection, label: string) {
  const active = state.form.direction === direction;
  const sign = direction === "expense" ? "-" : "+";

  return `
    <button
      class="segmented-option ${active ? "is-active" : ""}"
      type="button"
      data-direction="${direction}"
      aria-pressed="${active}"
    >
      <span aria-hidden="true">${sign}</span>
      ${label}
    </button>
  `;
}

function renderActiveView(dashboard: LedgerDashboard) {
  if (state.view === "analysis") {
    return renderAnalysisPanel(dashboard);
  }

  if (state.view === "approval") {
    return renderApprovalPanel(dashboard);
  }

  if (state.view === "audit") {
    return renderAuditPanel(dashboard);
  }

  return renderTransactionList(dashboard);
}

function renderAnalysisPanel(dashboard: LedgerDashboard) {
  const analysis =
    state.analysis?.ledgerId === dashboard.ledger.id &&
    state.analysis.months === state.analysisMonths
      ? state.analysis
      : undefined;
  const rangeControls = `
    <div class="analysis-range" role="group" aria-label="分析周期">
      ${renderAnalysisRangeButton(3)}
      ${renderAnalysisRangeButton(6)}
      ${renderAnalysisRangeButton(12)}
    </div>
  `;

  if (state.analysisLoading && !analysis) {
    return `
      <section class="analysis-view" aria-label="财务分析">
        <div class="analysis-heading">
          <div><span class="section-kicker">Analytics</span><h2>财务分析</h2></div>
          ${rangeControls}
        </div>
        <div class="analysis-state"><div class="spinner" aria-hidden="true"></div><p>正在汇总财务数据</p></div>
      </section>
    `;
  }

  if (state.analysisError && !analysis) {
    return `
      <section class="analysis-view" aria-label="财务分析">
        <div class="analysis-heading">
          <div><span class="section-kicker">Analytics</span><h2>财务分析</h2></div>
          ${rangeControls}
        </div>
        <div class="analysis-state">
          <p>${escapeHtml(state.analysisError)}</p>
          <button class="primary-button" id="retryAnalysisButton" type="button">重新加载</button>
        </div>
      </section>
    `;
  }

  if (!analysis) {
    return "";
  }

  const currency = analysis.currency;
  const cashFlowDelta = analysis.netCashFlowCents - analysis.previousNetCashFlowCents;
  const projectedBalance = analysis.currentBalanceCents - analysis.pendingPayment.amountCents;
  const averageMonthlyExpense = analysis.expenseCents / analysis.months;
  const coverageMonths =
    averageMonthlyExpense > 0
      ? Math.max(0, analysis.currentBalanceCents / averageMonthlyExpense)
      : undefined;
  const maxTrendValue = Math.max(
    1,
    ...analysis.trend.flatMap((point) => [point.incomeCents, point.expenseCents]),
  );
  const maxMemberExpense = Math.max(1, ...analysis.memberExpenses.map((item) => item.expenseCents));

  return `
    <section class="analysis-view" aria-label="财务分析">
      <div class="analysis-heading">
        <div>
          <span class="section-kicker">Analytics</span>
          <h2>财务分析</h2>
          <p>${formatPeriodMonth(analysis.periodStart)} 至 ${formatPeriodMonth(analysis.periodEnd)} · ${analysis.transactionCount} 笔实际收支</p>
        </div>
        ${rangeControls}
      </div>

      <div class="analysis-metrics">
        ${renderAnalysisMetric("当前余额", analysis.currentBalanceCents, currency, "全部账户")}
        ${renderAnalysisMetric("周期收入", analysis.incomeCents, currency, "实际入账")}
        ${renderAnalysisMetric("周期支出", analysis.expenseCents, currency, "已实际打款")}
        ${renderAnalysisMetric(
          "净现金流",
          analysis.netCashFlowCents,
          currency,
          `${cashFlowDelta >= 0 ? "较上期增加" : "较上期减少"} ${formatMoney(Math.abs(cashFlowDelta), currency)}`,
          analysis.netCashFlowCents >= 0 ? "positive" : "negative",
        )}
      </div>

      <section class="analysis-section analysis-signals" aria-label="财务判断">
        <div class="analysis-section-heading"><h3>资金判断</h3><span>当前</span></div>
        <div class="signal-list">
          <div><span>待付款后余额</span><strong class="${projectedBalance >= 0 ? "amount-in" : "amount-out"}">${formatMoney(projectedBalance, currency)}</strong></div>
          <p>已批准待打款全部付出后的账面余额</p>
          <div><span>静态资金覆盖</span><strong>${coverageMonths === undefined ? "暂无支出基线" : `${formatCompactNumber(coverageMonths)} 个月`}</strong></div>
          <p>按所选周期月均已支付支出估算，不计未来收入</p>
        </div>
      </section>

      <section class="analysis-section" aria-label="收支趋势">
        <div class="analysis-section-heading">
          <h3>月度收支趋势</h3>
          <div class="chart-legend"><span class="income-dot">收入</span><span class="expense-dot">支出</span></div>
        </div>
        <div class="cashflow-chart" role="img" aria-label="${analysis.months}个月收入和支出柱状图">
          ${analysis.trend
            .map(
              (point) => `
                <div class="chart-period" title="${escapeHtml(point.label)} 收入 ${escapeHtml(formatMoney(point.incomeCents, currency))}，支出 ${escapeHtml(formatMoney(point.expenseCents, currency))}">
                  <div class="bar-pair">
                    <span class="chart-bar income" style="--bar-height:${barHeight(point.incomeCents, maxTrendValue)}%"></span>
                    <span class="chart-bar expense" style="--bar-height:${barHeight(point.expenseCents, maxTrendValue)}%"></span>
                  </div>
                  <span>${escapeHtml(point.label)}</span>
                </div>
              `,
            )
            .join("")}
        </div>
      </section>

      <section class="analysis-section" aria-label="流程资金">
        <div class="analysis-section-heading"><h3>流程资金</h3><span>实时敞口</span></div>
        <div class="exposure-list">
          ${renderExposureRow("待审批", analysis.pendingApproval, currency, "尚未形成支出", "pending")}
          ${renderExposureRow("待打款", analysis.pendingPayment, currency, "已批准，尚未扣款", "committed")}
          ${renderExposureRow("待确认收款", analysis.paidPendingReceipt, currency, "已经计入支出", "paid")}
        </div>
      </section>

      <section class="analysis-section" aria-label="账户余额">
        <div class="analysis-section-heading"><h3>账户余额</h3><span>${analysis.accounts.length} 个账户</span></div>
        <div class="analysis-list">
          ${analysis.accounts
            .map(
              (account) => `<div class="analysis-list-row"><div><strong>${escapeHtml(account.name)}</strong><span>${escapeHtml(accountKindLabel(account.kind))}</span></div><strong>${formatMoney(account.balanceCents, currency)}</strong></div>`,
            )
            .join("")}
        </div>
      </section>

      <section class="analysis-section" aria-label="成员支出">
        <div class="analysis-section-heading"><h3>成员支出</h3><span>按已打款金额</span></div>
        <div class="member-spend-list">
          ${
            analysis.memberExpenses.length > 0
              ? analysis.memberExpenses
                  .map(
                    (member) => `
                      <div class="member-spend-row">
                        <div><strong>${escapeHtml(member.displayName)}</strong><span>${member.transactionCount} 笔</span></div>
                        <div class="member-spend-track"><span style="--spend-width:${Math.max(4, (member.expenseCents / maxMemberExpense) * 100)}%"></span></div>
                        <strong>${formatMoney(member.expenseCents, currency)}</strong>
                      </div>
                    `,
                  )
                  .join("")
              : `<p class="empty-copy">所选周期暂无已打款支出</p>`
          }
        </div>
      </section>

      <section class="analysis-section" aria-label="大额支出">
        <div class="analysis-section-heading"><h3>大额支出</h3><span>前 ${analysis.largestExpenses.length} 笔</span></div>
        <div class="analysis-list">
          ${
            analysis.largestExpenses.length > 0
              ? analysis.largestExpenses
                  .map(
                    (expense) => `<div class="analysis-list-row"><div><strong>${escapeHtml(expense.description)}</strong><span>${escapeHtml(expense.submittedBy)} · ${formatDate(expense.paidAt)}</span></div><strong class="amount-out">${formatMoney(expense.amountCents, currency)}</strong></div>`,
                  )
                  .join("")
              : `<p class="empty-copy">所选周期暂无已打款支出</p>`
          }
        </div>
      </section>

      <p class="analysis-updated">更新于 ${formatDate(analysis.generatedAt)} ${state.analysisLoading ? "· 刷新中" : ""}</p>
    </section>
  `;
}

function renderAnalysisRangeButton(months: AnalysisMonths) {
  const active = state.analysisMonths === months;
  return `<button class="filter-tab ${active ? "is-active" : ""}" type="button" data-analysis-months="${months}" aria-pressed="${active}" ${state.analysisLoading ? "disabled" : ""}>${months} 个月</button>`;
}

function renderAnalysisMetric(
  label: string,
  value: number,
  currency: string,
  detail: string,
  tone = "",
) {
  return `<article class="analysis-metric ${tone}"><span>${label}</span><strong>${formatMoney(value, currency)}</strong><p>${escapeHtml(detail)}</p></article>`;
}

function renderExposureRow(
  label: string,
  exposure: FinancialAnalysis["pendingApproval"],
  currency: string,
  detail: string,
  tone: string,
) {
  return `<div class="exposure-row ${tone}"><span class="exposure-mark" aria-hidden="true"></span><div><strong>${label}</strong><span>${exposure.count} 笔 · ${escapeHtml(detail)}</span></div><strong>${formatMoney(exposure.amountCents, currency)}</strong></div>`;
}

function barHeight(value: number, maximum: number) {
  return value > 0 ? Math.max(4, Math.round((value / maximum) * 100)) : 0;
}

function renderTransactionList(dashboard: LedgerDashboard) {
  const filtered = dashboard.recentTransactions.filter((transaction) => {
    if (state.filter === "pending") {
      return transaction.approvalState === "pending";
    }

    if (state.filter === "approved") {
      return transaction.approvalState === "approved";
    }

    if (state.filter === "rejected") {
      return transaction.approvalState === "rejected";
    }

    return true;
  });

  return `
    <section class="activity-panel" aria-label="流水列表">
      <div class="section-heading">
        <div>
          <span class="section-kicker">Activity</span>
          <h2>流水</h2>
        </div>
        <div class="filter-tabs" role="tablist" aria-label="流水筛选">
          ${renderFilterButton("all", "全部")}
          ${renderFilterButton("pending", "待审")}
          ${renderFilterButton("approved", "已入账")}
          ${renderFilterButton("rejected", "驳回")}
        </div>
      </div>
      <div class="activity-month-bar">
        <label for="activityMonthSelect">月份</label>
        <select id="activityMonthSelect">
          ${dashboard.availableTransactionMonths
            .map(
              (month) => `<option value="${escapeHtml(month)}" ${month === dashboard.selectedTransactionMonth ? "selected" : ""}>${escapeHtml(formatMonthLabel(month))}</option>`,
            )
            .join("")}
        </select>
        <span>${filtered.length} / ${dashboard.recentTransactions.length} 笔</span>
      </div>
      <div class="transaction-list">
        ${
          filtered.length > 0
            ? filtered.map((transaction) => renderTransactionRow(transaction, dashboard)).join("")
            : `<p class="empty-copy">暂无流水</p>`
        }
      </div>
    </section>
  `;
}

function renderFilterButton(filter: TransactionFilter, label: string) {
  const active = state.filter === filter;

  return `
    <button
      class="filter-tab ${active ? "is-active" : ""}"
      type="button"
      data-filter="${filter}"
      aria-selected="${active}"
    >
      ${label}
    </button>
  `;
}

function renderTransactionRow(
  transaction: LedgerDashboard["recentTransactions"][number],
  dashboard: LedgerDashboard,
) {
  const currency = dashboard.ledger.currency;
  const signedAmount = `${transaction.direction === "expense" ? "-" : "+"}${formatMoney(
    transaction.amountCents,
    currency,
  )}`;

  return `
    <article class="transaction-row">
      <div class="row-main">
        <div>
          <h3>${escapeHtml(transaction.title)}</h3>
          <p>${escapeHtml(transaction.accountName)} · ${escapeHtml(transaction.categoryName)}</p>
        </div>
        <strong class="${transaction.direction === "expense" ? "amount-out" : "amount-in"}">
          ${signedAmount}
        </strong>
      </div>
      <div class="row-meta">
        <span>${formatDate(transaction.occurredAt)}</span>
        <span class="status-chip ${statusClass(transaction.approvalState)}">
          ${transactionStateLabel(transaction)}
        </span>
      </div>
      ${renderTransactionActions(transaction, dashboard)}
    </article>
  `;
}

function renderTransactionActions(
  transaction: LedgerDashboard["recentTransactions"][number],
  dashboard: LedgerDashboard,
) {
  const canMarkPaid =
    dashboard.ledger.role === "business_owner" && transaction.paymentState === "pending_payment";
  const canConfirmReceipt =
    transaction.paymentState === "paid_pending_receipt" &&
    transaction.createdByUserId === state.currentUser?.id;
  if (!canMarkPaid && !canConfirmReceipt) {
    return "";
  }

  return `<div class="transaction-actions">
    ${
      canMarkPaid
        ? `<button class="secondary-button" type="button" data-payment-action="mark-paid" data-transaction-id="${escapeHtml(transaction.id)}">标记已打款</button>`
        : ""
    }
    ${
      canConfirmReceipt
        ? `<button class="primary-button" type="button" data-payment-action="confirm-receipt" data-transaction-id="${escapeHtml(transaction.id)}">确认收到款项</button>`
        : ""
    }
  </div>`;
}

function renderApprovalPanel(dashboard: LedgerDashboard) {
  return `
    <section class="activity-panel" aria-label="审批入口">
      <div class="section-heading">
        <div>
          <span class="section-kicker">Approval</span>
          <h2>审批</h2>
        </div>
        <span class="count-badge">${dashboard.approvalQueue.length}</span>
      </div>
      <div class="approval-list">
        ${
          dashboard.approvalQueue.length > 0
            ? dashboard.approvalQueue
                .map((item) => renderApprovalRow(item, dashboard.ledger.currency))
                .join("")
            : `<p class="empty-copy">暂无待审批流水</p>`
        }
      </div>
    </section>
  `;
}

function renderApprovalRow(item: LedgerDashboard["approvalQueue"][number], currency: string) {
  const signedAmount = `${item.direction === "expense" ? "-" : "+"}${formatMoney(item.amountCents, currency)}`;
  const processing = state.pendingAction === item.transactionId;
  const disabled = state.loading ? "disabled" : "";
  const decisionControls = item.canDecide
    ? `
      <div class="approval-actions">
        <button
          class="secondary-button"
          type="button"
          data-approval-transaction-id="${escapeHtml(item.transactionId)}"
          data-approval-decision="approve"
          ${disabled}
        >
          ${processing ? "处理中" : "批准"}
        </button>
        <button
          class="danger-button"
          type="button"
          data-approval-transaction-id="${escapeHtml(item.transactionId)}"
          data-approval-decision="reject"
          ${disabled}
        >
          驳回
        </button>
      </div>
    `
    : `<p class="approval-note">仅其他老板可审批，不能审批自己提交的流水</p>`;

  return `
    <article class="approval-row">
      <div class="approval-main">
        <div>
          <h3>${escapeHtml(item.title)}</h3>
          <p>${escapeHtml(item.submittedBy)} · ${formatDate(item.submittedAt)}</p>
        </div>
        <strong>${signedAmount}</strong>
      </div>
      ${decisionControls}
    </article>
  `;
}

function renderAuditPanel(dashboard: LedgerDashboard) {
  return `
    <section class="activity-panel" aria-label="审计入口">
      <div class="section-heading">
        <div>
          <span class="section-kicker">Audit</span>
          <h2>审计</h2>
        </div>
        <span class="count-badge">${dashboard.auditTrail.length}</span>
      </div>
      <div class="audit-list">
        ${
          dashboard.auditTrail.length > 0
            ? dashboard.auditTrail
                .map(
                  (item) => `
                    <article class="audit-row">
                      <span class="audit-dot" aria-hidden="true"></span>
                      <div>
                        <h3>${escapeHtml(item.summary)}</h3>
                        <p>${escapeHtml(item.actorName)} · ${auditActionLabel(item.action)} · ${formatDate(
                          item.createdAt,
                        )}</p>
                        <p>资源 #${escapeHtml(item.resourceId.slice(0, 8))}</p>
                      </div>
                    </article>
                  `,
                )
                .join("")
            : `<p class="empty-copy">暂无审计记录</p>`
        }
      </div>
    </section>
  `;
}

function renderBottomNav(dashboard: LedgerDashboard) {
  const canViewAnalysis =
    dashboard.ledger.kind === "organization" && dashboard.ledger.role === "business_owner";
  const navCount = canViewAnalysis ? 4 : 2;
  return `
    <nav class="bottom-nav" aria-label="主导航" style="--nav-count:${navCount}">
      ${renderNavButton("activity", "流水", dashboard.recentTransactions.length)}
      ${canViewAnalysis ? renderNavButton("analysis", "分析") : ""}
      ${dashboard.ledger.role === "business_owner" ? renderNavButton("approval", "审批", dashboard.approvalQueue.length) : ""}
      ${renderNavButton("audit", "审计", dashboard.auditTrail.length)}
    </nav>
  `;
}

function renderNavButton(view: ViewMode, label: string, count?: number) {
  const active = state.view === view;

  return `
    <button class="nav-button ${active ? "is-active" : ""}" type="button" data-view-target="${view}">
      <span class="nav-icon" aria-hidden="true">${label.slice(0, 1)}</span>
      <span>${label}</span>
      <span class="nav-count ${count === undefined ? "is-empty" : ""}">${count ?? ""}</span>
    </button>
  `;
}

function renderLoading() {
  return `
    <section class="state-panel">
      <div class="spinner" aria-hidden="true"></div>
      <p>正在加载账本</p>
    </section>
  `;
}

function renderError() {
  return `
    <section class="state-panel">
      <p>${escapeHtml(state.error ?? "加载失败")}</p>
      <button class="primary-button" type="button" id="retryButton">重试</button>
    </section>
  `;
}

function renderEmptyState() {
  return `
    <section class="state-panel">
      <p>暂无账本</p>
    </section>
  `;
}

function bindEvents() {
  app.querySelector<HTMLFormElement>("#authForm")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void submitAuthForm();
  });

  app.querySelector<HTMLInputElement>("#authIdentifier")?.addEventListener("input", (event) => {
    const target = event.currentTarget as HTMLInputElement;
    state.authForm.identifier = target.value;
  });

  app.querySelector<HTMLInputElement>("#authPassword")?.addEventListener("input", (event) => {
    const target = event.currentTarget as HTMLInputElement;
    state.authForm.password = target.value;
  });

  app.querySelector<HTMLButtonElement>("#userMenuButton")?.addEventListener("click", (event) => {
    event.stopPropagation();
    state.userMenuOpen = !state.userMenuOpen;
    if (state.userMenuOpen) {
      syncProfileFormFromUser();
    } else {
      state.profileEditing = false;
    }
    render();
  });

  app.querySelector<HTMLButtonElement>("#editProfileButton")?.addEventListener("click", () => {
    state.profileEditing = true;
    syncProfileFormFromUser();
    render();
  });

  app.querySelector<HTMLButtonElement>("#cancelProfileEdit")?.addEventListener("click", () => {
    state.profileEditing = false;
    syncProfileFormFromUser();
    render();
  });

  app.querySelector<HTMLInputElement>("#profileDisplayName")?.addEventListener("input", (event) => {
    const target = event.currentTarget as HTMLInputElement;
    state.profileForm.displayName = target.value;
  });

  app.querySelector<HTMLFormElement>("#profileForm")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void saveProfile();
  });

  app.querySelector<HTMLButtonElement>("[data-user-menu-action='logout']")?.addEventListener("click", () => {
    void logout();
  });

  app.querySelectorAll<HTMLButtonElement>("[data-ledger-id]").forEach((button) => {
    button.addEventListener("click", () => {
      const ledgerId = button.dataset.ledgerId;
      if (ledgerId && ledgerId !== state.activeLedgerId) {
        void switchLedger(ledgerId);
      }
    });
  });

  app.querySelector<HTMLFormElement>("#quickEntryForm")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void submitQuickEntry();
  });

  app.querySelector<HTMLInputElement>("#amountInput")?.addEventListener("input", (event) => {
    const target = event.currentTarget as HTMLInputElement;
    state.form.amount = target.value;
  });

  app.querySelector<HTMLSelectElement>("#accountSelect")?.addEventListener("change", (event) => {
    const target = event.currentTarget as HTMLSelectElement;
    state.form.accountId = target.value;
  });

  app.querySelector<HTMLSelectElement>("#categorySelect")?.addEventListener("change", (event) => {
    const target = event.currentTarget as HTMLSelectElement;
    state.form.categoryId = target.value;
  });

  app.querySelector<HTMLButtonElement>("#addCategoryButton")?.addEventListener("click", () => {
    state.categoryEditing = !state.categoryEditing;
    state.categoryName = "";
    render();
    app.querySelector<HTMLInputElement>("#categoryNameInput")?.focus();
  });

  app.querySelector<HTMLInputElement>("#categoryNameInput")?.addEventListener("input", (event) => {
    state.categoryName = (event.currentTarget as HTMLInputElement).value;
  });

  app.querySelector<HTMLInputElement>("#categoryNameInput")?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void saveCategory();
    }
  });

  app.querySelector<HTMLButtonElement>("#saveCategoryButton")?.addEventListener("click", () => {
    void saveCategory();
  });

  app.querySelector<HTMLButtonElement>("#cancelCategoryButton")?.addEventListener("click", () => {
    state.categoryEditing = false;
    state.categoryName = "";
    render();
  });

  app.querySelector<HTMLInputElement>("#memoInput")?.addEventListener("input", (event) => {
    const target = event.currentTarget as HTMLInputElement;
    state.form.memo = target.value;
  });

  app.querySelector<HTMLInputElement>("#approvalToggle")?.addEventListener("change", (event) => {
    if (state.dashboard?.ledger.kind === "organization") {
      state.form.submitForApproval = true;
      render();
      return;
    }
    const target = event.currentTarget as HTMLInputElement;
    state.form.submitForApproval = target.checked;
  });

  app.querySelectorAll<HTMLButtonElement>("[data-direction]").forEach((button) => {
    button.addEventListener("click", () => {
      const direction = button.dataset.direction as TransactionDirection;
      state.form.direction = direction;
      state.form.categoryId = categoriesForDirection(direction)[0]?.id ?? "";
      render();
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-filter]").forEach((button) => {
    button.addEventListener("click", () => {
      state.filter = button.dataset.filter as TransactionFilter;
      render();
    });
  });

  app.querySelector<HTMLSelectElement>("#activityMonthSelect")?.addEventListener("change", (event) => {
    void loadActivityMonth((event.currentTarget as HTMLSelectElement).value);
  });

  app.querySelectorAll<HTMLButtonElement>("[data-view-target]").forEach((button) => {
    button.addEventListener("click", () => {
      void changeView(button.dataset.viewTarget as ViewMode);
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-analysis-months]").forEach((button) => {
    button.addEventListener("click", () => {
      const months = Number(button.dataset.analysisMonths);
      if (months === 3 || months === 6 || months === 12) {
        state.analysisMonths = months;
        state.analysis = undefined;
        void loadFinancialAnalysis(true);
      }
    });
  });

  app.querySelector<HTMLButtonElement>("#retryAnalysisButton")?.addEventListener("click", () => {
    void loadFinancialAnalysis(true);
  });

  app.querySelectorAll<HTMLButtonElement>("[data-approval-decision]").forEach((button) => {
    button.addEventListener("click", () => {
      const transactionId = button.dataset.approvalTransactionId;
      const decision = button.dataset.approvalDecision;
      if ((decision === "approve" || decision === "reject") && transactionId) {
        void decideApproval(transactionId, decision);
      }
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-payment-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const transactionId = button.dataset.transactionId;
      const action = button.dataset.paymentAction;
      if (transactionId && (action === "mark-paid" || action === "confirm-receipt")) {
        void updatePaymentState(transactionId, action);
      }
    });
  });

  app.querySelector<HTMLButtonElement>("#retryButton")?.addEventListener("click", () => {
    void loadInitialState();
  });
}

function closeUserMenu() {
  state.userMenuOpen = false;
  state.profileEditing = false;
  syncProfileFormFromUser();
  render();
}

async function saveProfile() {
  if (state.pendingAction) {
    return;
  }

  const displayName = state.profileForm.displayName.trim();
  if (!displayName) {
    showToast("请输入显示名");
    return;
  }

  try {
    state.pendingAction = "profile";
    render();
    const draft: UpdateProfileDraft = { displayName };
    const session = await cloudLedgerApi.updateProfile(draft);
    state.currentUser = session.currentUser;
    state.cloudStatus = session.cloudStatus;
    state.profileEditing = false;
    state.userMenuOpen = true;
    syncProfileFormFromUser();
    await refreshRemoteState({ silent: true, allowPending: true });
    showToast("账号信息已更新");
  } catch (error) {
    showToast(friendlyError(error, "保存账号信息失败"));
  } finally {
    state.pendingAction = undefined;
    render();
  }
}

async function decideApproval(transactionId: string, decision: "approve" | "reject") {
  if (state.pendingAction) {
    return;
  }

  const decisionNote =
    decision === "reject" ? window.prompt("请输入驳回原因")?.trim() : undefined;
  if (decision === "reject" && !decisionNote) {
    showToast("请输入驳回原因");
    return;
  }

  try {
    state.loading = true;
    state.pendingAction = transactionId;
    render();
    const transaction = await cloudLedgerApi.decideApproval(transactionId, decision, decisionNote);
    state.analysis = undefined;
    state.activityMonth = transactionMonthKey(transaction.occurredAt);
    await refreshDashboard();
    state.view = "activity";
    state.filter = decision === "approve" ? "approved" : "rejected";
    showToast(
      decision === "approve"
        ? transaction.paymentState === "pending_payment"
          ? "已批准，等待打款"
          : "已批准入账"
        : "已驳回流水",
    );
  } catch (error) {
    showToast(friendlyError(error, "审批失败"));
  } finally {
    state.loading = false;
    state.pendingAction = undefined;
    render();
  }
}

async function updatePaymentState(
  transactionId: string,
  action: "mark-paid" | "confirm-receipt",
) {
  if (state.pendingAction) {
    return;
  }
  try {
    state.loading = true;
    state.pendingAction = transactionId;
    render();
    const transaction = action === "mark-paid"
      ? await cloudLedgerApi.markTransactionPaid(transactionId)
      : await cloudLedgerApi.confirmTransactionReceipt(transactionId);
    state.analysis = undefined;
    state.activityMonth = transactionMonthKey(transaction.occurredAt);
    await refreshDashboard();
    state.view = "activity";
    showToast(action === "mark-paid" ? "已标记打款，等待申请人确认" : "已确认收到款项");
  } catch (error) {
    showToast(friendlyError(error, action === "mark-paid" ? "标记打款失败" : "确认收款失败"));
  } finally {
    state.loading = false;
    state.pendingAction = undefined;
    render();
  }
}

async function saveCategory() {
  const dashboard = state.dashboard;
  const name = state.categoryName.trim();
  if (!dashboard || state.pendingAction) {
    return;
  }
  if (!name) {
    showToast("请输入分类名称");
    return;
  }
  if (Array.from(name).length > 24) {
    showToast("分类名称不能超过 24 个字符");
    return;
  }

  try {
    state.pendingAction = "create-category";
    render();
    const category = await cloudLedgerApi.createCategory(
      dashboard.ledger.id,
      name,
      state.form.direction,
    );
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(
      dashboard.ledger.id,
      state.activityMonth,
    );
    state.form.categoryId = category.id;
    state.categoryEditing = false;
    state.categoryName = "";
    showToast(`已添加分类：${category.name}`);
  } catch (error) {
    showToast(friendlyError(error, "添加分类失败"));
  } finally {
    state.pendingAction = undefined;
    render();
  }
}

async function submitQuickEntry() {
  const dashboard = state.dashboard;
  if (!dashboard) {
    return;
  }

  const amountCents = parseAmountToCents(state.form.amount);
  if (!amountCents || amountCents <= 0) {
    showToast("请输入有效金额");
    return;
  }
  if (!state.form.accountId) {
    showToast("请选择账户");
    return;
  }
  if (!state.form.categoryId) {
    showToast("请选择分类");
    return;
  }

  const draft: NewTransactionDraft = {
    ledgerId: dashboard.ledger.id,
    occurredAt: new Date().toISOString(),
    direction: state.form.direction,
    amountCents,
    accountId: state.form.accountId,
    categoryId: state.form.categoryId,
    memo: state.form.memo.trim() || undefined,
    submitForApproval: dashboard.ledger.kind === "organization" || state.form.submitForApproval,
  };

  try {
    state.loading = true;
    state.pendingAction = "create";
    render();
    const transaction = await cloudLedgerApi.createTransaction(draft);
    state.analysis = undefined;
    state.form.amount = "";
    state.form.memo = "";
    state.activityMonth = currentMonthKey();
    await refreshDashboard();
    state.view = "activity";
    showToast(
      transaction.paymentState === "pending_payment"
        ? "已自动批准，等待打款"
        : transaction.approvalState === "pending"
          ? "已提交审批"
          : "已保存流水",
    );
  } catch (error) {
    showToast(friendlyError(error, "保存失败"));
  } finally {
    state.loading = false;
    state.pendingAction = undefined;
    render();
  }
}

function showToast(message: string) {
  state.toast = message;
  render();
  window.setTimeout(() => {
    if (state.toast === message) {
      state.toast = undefined;
      render();
    }
  }, 2200);
}

function friendlyError(error: unknown, fallback: string) {
  const message = error instanceof Error ? error.message : String(error || fallback);

  if (message.includes("rejection reason is required")) {
    return "请输入驳回原因";
  }
  if (message.includes("submitter cannot approve")) {
    return "不能审批自己提交的流水";
  }
  if (message.includes("not pending approval")) {
    return "这笔流水已不在待审批状态";
  }
  if (message.includes("not waiting for payment")) {
    return "这笔流水已不在当前付款状态";
  }
  if (message.includes("only the applicant can confirm receipt")) {
    return "只有申请人可以确认收到款项";
  }
  if (message.includes("not authorized")) {
    return "当前账号没有权限执行此操作";
  }
  if (message.includes("currency must match")) {
    return "流水币种必须和账户币种一致";
  }
  if (message.includes("transfer transactions are not supported")) {
    return "当前 MVP 暂不支持转账流水";
  }
  if (message.includes("transaction was not found")) {
    return "流水不存在或已被移除";
  }
  if (message.includes("category already exists")) {
    return "这个分类已经存在";
  }
  if (message.includes("category name is required")) {
    return "分类名称不能为空且不能超过 24 个字符";
  }
  if (message.includes("category direction does not match")) {
    return "分类与当前收支方向不匹配";
  }
  if (message.includes("transaction month must use")) {
    return "流水月份格式无效";
  }
  if (message.includes("organization admin accounts cannot use the business app")) {
    return "组织管理员账号只能登录后台，不能用于前台业务";
  }
  if (message.includes("too many login attempts")) {
    return "登录尝试过多，请稍后重试";
  }
  if (message.includes("invalid credentials")) {
    return "账号或密码错误";
  }

  return message || fallback;
}

function parseAmountToCents(value: string): number | undefined {
  const normalized = value.trim().replace(",", ".");
  if (!/^\d+(\.\d{0,2})?$/.test(normalized)) {
    return undefined;
  }

  return Math.round(Number(normalized) * 100);
}

function formatMoney(cents: number, currency: string) {
  const formatter =
    moneyFormatterCache.get(currency) ??
    new Intl.NumberFormat("zh-CN", {
      style: "currency",
      currency,
      currencyDisplay: "narrowSymbol",
      minimumFractionDigits: 2,
    });

  moneyFormatterCache.set(currency, formatter);
  return formatter.format(cents / 100);
}

function formatDate(value?: string) {
  if (!value) {
    return "未同步";
  }

  return dateFormatter.format(new Date(value));
}

function formatPeriodMonth(value: string) {
  return periodMonthFormatter.format(new Date(value));
}

function currentMonthKey() {
  return transactionMonthKey(new Date().toISOString());
}

function transactionMonthKey(value: string) {
  return value.slice(0, 7);
}

function formatMonthLabel(month: string) {
  const [year, monthNumber] = month.split("-");
  return `${year}年${Number(monthNumber)}月`;
}

function formatCompactNumber(value: number) {
  return new Intl.NumberFormat("zh-CN", {
    maximumFractionDigits: 1,
  }).format(value);
}

function accountKindLabel(kind: string) {
  const labels: Record<string, string> = {
    cash: "现金",
    bank: "银行账户",
    wechat: "微信",
    alipay: "支付宝",
    company: "公司账户",
    wallet: "电子钱包",
    credit: "信用账户",
    receivable: "应收账户",
    payable: "应付账户",
  };
  return labels[kind] ?? kind;
}

function ledgerKindLabel(kind: Ledger["kind"]) {
  return kind === "private" ? "私人账本" : "公共账本";
}

function roleLabel(role: Ledger["role"]) {
  const labels: Record<Ledger["role"], string> = {
    owner: "所有者",
    admin: "管理员",
    business_owner: "老板",
    employee: "员工",
  };

  return labels[role];
}

function transactionStateLabel(transaction: LedgerDashboard["recentTransactions"][number]) {
  if (transaction.approvalState === "pending") return "待老板审批";
  if (transaction.approvalState === "rejected") return "已驳回";
  if (transaction.approvalState === "deleted") return "已作废";
  if (transaction.approvalState === "draft") return "草稿";
  if (transaction.paymentState === "pending_payment") return "已批准待打款";
  if (transaction.paymentState === "paid_pending_receipt") return "已打款待确认";
  if (transaction.paymentState === "received") return "已完成";
  return "已入账";
}

function statusClass(stateValue: ApprovalState) {
  const classes: Record<ApprovalState, string> = {
    draft: "is-draft",
    pending: "is-pending",
    approved: "is-approved",
    rejected: "is-rejected",
    deleted: "is-deleted",
  };

  return classes[stateValue];
}

function auditActionLabel(action: LedgerDashboard["auditTrail"][number]["action"]) {
  const labels: Record<LedgerDashboard["auditTrail"][number]["action"], string> = {
    transaction_created: "创建",
    transaction_submitted: "提交",
    transaction_approved: "批准",
    transaction_rejected: "驳回",
    transaction_paid: "打款",
    transaction_received: "确认收款",
    transaction_auto_approved: "自动批准",
    transaction_deleted: "删除",
  };

  return labels[action];
}

function userInitial(displayName: string) {
  return Array.from(displayName.trim())[0]?.toUpperCase() ?? "账";
}

function escapeHtml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}
