import { cloudLedgerApi } from "./api/cloudLedger";
import "./styles.css";
import type {
  ApprovalState,
  Category,
  Ledger,
  LedgerDashboard,
  NewTransactionDraft,
  TransactionDirection,
  UserAccount,
  UserSession,
} from "./types";

type ViewMode = "activity" | "approval" | "audit";
type TransactionFilter = "all" | "pending" | "approved" | "rejected";

interface QuickEntryForm {
  direction: TransactionDirection;
  amount: string;
  accountId: string;
  categoryId: string;
  memo: string;
  submitForApproval: boolean;
}

interface AppState {
  users: UserAccount[];
  ledgers: Ledger[];
  dashboard?: LedgerDashboard;
  activeLedgerId?: string;
  activeUserId?: string;
  cloudStatus: UserSession["cloudStatus"];
  loading: boolean;
  pendingAction?: string;
  error?: string;
  view: ViewMode;
  filter: TransactionFilter;
  form: QuickEntryForm;
  toast?: string;
}

const appRoot = document.querySelector<HTMLDivElement>("#app");

if (!appRoot) {
  throw new Error("Missing #app root");
}

const app = appRoot;

const state: AppState = {
  users: [],
  ledgers: [],
  cloudStatus: {
    state: "checking",
    label: "云端检测中",
  },
  loading: true,
  view: "activity",
  filter: "all",
  form: {
    direction: "expense",
    amount: "",
    accountId: "",
    categoryId: "",
    memo: "",
    submitForApproval: false,
  },
};

const moneyFormatterCache = new Map<string, Intl.NumberFormat>();
const dateFormatter = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});

void loadInitialState();

async function loadInitialState() {
  try {
    state.loading = true;
    render();
    const session = await cloudLedgerApi.getUserSession();
    const ledgers = await cloudLedgerApi.listLedgers();
    const activeLedgerId = pickDefaultLedgerId(ledgers);

    state.users = session.users;
    state.activeUserId = session.currentUser.id;
    state.cloudStatus = session.cloudStatus;
    state.ledgers = ledgers;
    state.activeLedgerId = activeLedgerId;
    state.dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(activeLedgerId)
      : undefined;
    resetFormForDashboard();
    state.error = undefined;
  } catch (error) {
    state.error = friendlyError(error, "加载失败");
  } finally {
    state.loading = false;
    render();
  }
}

async function switchUser(userId: string) {
  try {
    state.loading = true;
    state.activeUserId = userId;
    render();
    await cloudLedgerApi.switchUser(userId);
    const session = await cloudLedgerApi.getUserSession();
    const ledgers = await cloudLedgerApi.listLedgers();
    const activeLedgerId = pickDefaultLedgerId(ledgers);

    state.users = session.users;
    state.activeUserId = session.currentUser.id;
    state.cloudStatus = session.cloudStatus;
    state.ledgers = ledgers;
    state.activeLedgerId = activeLedgerId;
    state.dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(activeLedgerId)
      : undefined;
    state.view = "activity";
    state.filter = "all";
    resetFormForDashboard();
    state.error = undefined;
  } catch (error) {
    state.error = friendlyError(error, "切换账号失败");
  } finally {
    state.loading = false;
    render();
  }
}

async function switchLedger(ledgerId: string) {
  try {
    state.loading = true;
    state.activeLedgerId = ledgerId;
    render();
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(ledgerId);
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

async function refreshDashboard() {
  if (!state.activeLedgerId) {
    return;
  }

  const session = await cloudLedgerApi.getUserSession();
  state.cloudStatus = session.cloudStatus;
  state.dashboard = await cloudLedgerApi.getLedgerDashboard(state.activeLedgerId);
  resetFormForDashboard({ preserveAmount: true });
}

function resetFormForDashboard(options: { preserveAmount?: boolean } = {}) {
  const dashboard = state.dashboard;
  if (!dashboard) {
    return;
  }

  const firstAccount = dashboard.accounts[0];
  const categories = categoriesForDirection(state.form.direction);

  state.form = {
    ...state.form,
    amount: options.preserveAmount ? state.form.amount : "",
    accountId: firstAccount?.id ?? "",
    categoryId: categories[0]?.id ?? "",
    memo: options.preserveAmount ? state.form.memo : "",
    submitForApproval: dashboard.ledger.kind === "organization",
  };
}

function categoriesForDirection(direction: TransactionDirection): Category[] {
  return state.dashboard?.categories.filter((category) => category.direction === direction) ?? [];
}

function render() {
  const dashboard = state.dashboard;
  const ledger = dashboard?.ledger;

  app.innerHTML = `
    <main
      class="app-shell"
      data-active-user-id="${escapeHtml(state.activeUserId ?? "")}"
      data-active-ledger-id="${escapeHtml(state.activeLedgerId ?? "")}"
      data-cloud-state="${escapeHtml(state.cloudStatus.state)}"
    >
      ${renderTopBar()}
      ${
        state.loading && !dashboard
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
      <div class="top-controls">
        <span class="cloud-chip ${state.cloudStatus.state}">${escapeHtml(state.cloudStatus.label)}</span>
        <div class="control-block account-picker" id="userSelect" role="group" aria-label="账号切换">
          <span>账号</span>
          <div class="switcher-row">
            ${state.users.map(renderUserSwitchButton).join("")}
          </div>
        </div>
        <div class="control-block ledger-picker" id="ledgerSelect" role="group" aria-label="账本切换">
          <span>账本</span>
          <div class="switcher-row">
            ${state.ledgers.map(renderLedgerSwitchButton).join("")}
          </div>
        </div>
      </div>
    </header>
  `;
}

function renderUserSwitchButton(user: UserAccount) {
  const active = user.id === state.activeUserId;

  return `
    <button
      class="switcher-button ${active ? "is-active" : ""}"
      type="button"
      data-user-id="${escapeHtml(user.id)}"
      aria-pressed="${active}"
    >
      ${escapeHtml(user.displayName)}
    </button>
  `;
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
    ${renderQuickEntry(dashboard)}
    ${renderActiveView(dashboard)}
  `;
}

function renderBalancePanel(dashboard: LedgerDashboard) {
  const { ledger } = dashboard;
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
        <span class="context-pill">${ledger.auditUnreadCount} 审计</span>
      </div>
      <p class="sync-line">最近流水 ${formatDate(ledger.lastSyncedAt)}</p>
    </section>
  `;
}

function renderQuickEntry(dashboard: LedgerDashboard) {
  const form = state.form;
  const categories = categoriesForDirection(form.direction);
  const accountOptions = dashboard.accounts
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
        <label>
          <span>分类</span>
          <select id="categorySelect">${categoryOptions}</select>
        </label>
      </div>

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
        <span>${dashboard.ledger.kind === "organization" ? "公账自动审批" : "提交审批"}</span>
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
  if (state.view === "approval") {
    return renderApprovalPanel(dashboard);
  }

  if (state.view === "audit") {
    return renderAuditPanel(dashboard);
  }

  return renderTransactionList(dashboard);
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
      <div class="transaction-list">
        ${
          filtered.length > 0
            ? filtered.map((transaction) => renderTransactionRow(transaction, dashboard.ledger.currency)).join("")
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
  currency: string,
) {
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
          ${approvalStateLabel(transaction.approvalState)}
        </span>
      </div>
    </article>
  `;
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
    : `<p class="approval-note">仅审批人或所有者可处理，且不能审批自己提交的流水</p>`;

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
  return `
    <nav class="bottom-nav" aria-label="主导航">
      ${renderNavButton("activity", "流水", dashboard.recentTransactions.length)}
      ${renderNavButton("approval", "审批", dashboard.approvalQueue.length)}
      ${renderNavButton("audit", "审计", dashboard.auditTrail.length)}
    </nav>
  `;
}

function renderNavButton(view: ViewMode, label: string, count: number) {
  const active = state.view === view;

  return `
    <button class="nav-button ${active ? "is-active" : ""}" type="button" data-view-target="${view}">
      <span class="nav-icon" aria-hidden="true">${label.slice(0, 1)}</span>
      <span>${label}</span>
      <span class="nav-count">${count}</span>
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
  app.querySelectorAll<HTMLButtonElement>("[data-user-id]").forEach((button) => {
    button.addEventListener("click", () => {
      const userId = button.dataset.userId;
      if (userId && userId !== state.activeUserId) {
        void switchUser(userId);
      }
    });
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

  app.querySelectorAll<HTMLButtonElement>("[data-view-target]").forEach((button) => {
    button.addEventListener("click", () => {
      state.view = button.dataset.viewTarget as ViewMode;
      render();
    });
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

  app.querySelector<HTMLButtonElement>("#retryButton")?.addEventListener("click", () => {
    void loadInitialState();
  });
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
    await cloudLedgerApi.decideApproval(transactionId, decision, decisionNote);
    await refreshDashboard();
    state.view = "activity";
    state.filter = decision === "approve" ? "approved" : "rejected";
    showToast(decision === "approve" ? "已批准入账" : "已驳回流水");
  } catch (error) {
    showToast(friendlyError(error, "审批失败"));
  } finally {
    state.loading = false;
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
    await cloudLedgerApi.createTransaction(draft);
    state.form.amount = "";
    state.form.memo = "";
    await refreshDashboard();
    state.view = "activity";
    showToast(state.form.submitForApproval ? "已提交审批" : "已保存流水");
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

function ledgerKindLabel(kind: Ledger["kind"]) {
  return kind === "private" ? "私人账本" : "公共账本";
}

function roleLabel(role: Ledger["role"]) {
  const labels: Record<Ledger["role"], string> = {
    owner: "所有者",
    admin: "管理员",
    accountant: "会计",
    approver: "审批",
    member: "成员",
    auditor: "审计员",
    viewer: "只读",
  };

  return labels[role];
}

function approvalStateLabel(stateValue: ApprovalState) {
  const labels: Record<ApprovalState, string> = {
    draft: "草稿",
    pending: "审批中",
    approved: "已入账",
    rejected: "已驳回",
    deleted: "已删除",
  };

  return labels[stateValue];
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
    transaction_deleted: "删除",
  };

  return labels[action];
}

function escapeHtml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}
