import { cloudLedgerApi } from "./api/cloudLedger";
import "./styles.css";
import type {
  ApprovalState,
  Category,
  Ledger,
  LedgerDashboard,
  NewTransactionDraft,
  TransactionDirection,
} from "./types";

type ViewMode = "activity" | "approval" | "audit";
type TransactionFilter = "all" | "pending" | "approved";

interface QuickEntryForm {
  direction: TransactionDirection;
  amount: string;
  accountId: string;
  categoryId: string;
  memo: string;
  submitForApproval: boolean;
}

interface AppState {
  ledgers: Ledger[];
  dashboard?: LedgerDashboard;
  activeLedgerId?: string;
  loading: boolean;
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
  ledgers: [],
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
    const ledgers = await cloudLedgerApi.listLedgers();
    const activeLedgerId = ledgers[0]?.id;

    state.ledgers = ledgers;
    state.activeLedgerId = activeLedgerId;
    state.dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(activeLedgerId)
      : undefined;
    resetFormForDashboard();
    state.error = undefined;
  } catch (error) {
    state.error = error instanceof Error ? error.message : "加载失败";
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
    state.error = error instanceof Error ? error.message : "切换账本失败";
  } finally {
    state.loading = false;
    render();
  }
}

async function refreshDashboard() {
  if (!state.activeLedgerId) {
    return;
  }

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
    <main class="app-shell">
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

function renderTopBar() {
  const ledger = state.dashboard?.ledger;
  const ledgerOptions = state.ledgers
    .map(
      (item) => `
        <option value="${escapeHtml(item.id)}" ${item.id === state.activeLedgerId ? "selected" : ""}>
          ${escapeHtml(item.name)}
        </option>
      `,
    )
    .join("");

  return `
    <header class="top-bar">
      <div class="brand-block">
        <span class="brand-mark" aria-hidden="true">CL</span>
        <div>
          <h1>CloudLedger</h1>
          <p>${ledger ? `${ledgerKindLabel(ledger.kind)} · ${roleLabel(ledger.role)}` : "移动账本"}</p>
        </div>
      </div>
      <label class="ledger-picker">
        <span>账本</span>
        <select id="ledgerSelect" ${state.ledgers.length === 0 ? "disabled" : ""}>
          ${ledgerOptions}
        </select>
      </label>
    </header>
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
      <p class="sync-line">同步 ${formatDate(ledger.lastSyncedAt)}</p>
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
        <input id="approvalToggle" type="checkbox" ${form.submitForApproval ? "checked" : ""} />
        <span>提交审批</span>
      </label>

      <button class="primary-button" type="submit">
        <span aria-hidden="true">+</span>
        保存流水
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
                .map(
                  (item) => `
                    <article class="approval-row">
                      <div>
                        <h3>${escapeHtml(item.title)}</h3>
                        <p>${escapeHtml(item.submittedBy)} · ${formatDate(item.submittedAt)}</p>
                      </div>
                      <strong>${item.direction === "expense" ? "-" : "+"}${formatMoney(
                        item.amountCents,
                        dashboard.ledger.currency,
                      )}</strong>
                    </article>
                  `,
                )
                .join("")
            : `<p class="empty-copy">暂无待审批流水</p>`
        }
      </div>
    </section>
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
  app.querySelector<HTMLSelectElement>("#ledgerSelect")?.addEventListener("change", (event) => {
    const target = event.currentTarget as HTMLSelectElement;
    void switchLedger(target.value);
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

  app.querySelector<HTMLButtonElement>("#retryButton")?.addEventListener("click", () => {
    void loadInitialState();
  });
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
    submitForApproval: state.form.submitForApproval,
  };

  try {
    await cloudLedgerApi.createTransaction(draft);
    state.form.amount = "";
    state.form.memo = "";
    await refreshDashboard();
    state.view = "activity";
    showToast(state.form.submitForApproval ? "已提交审批" : "已保存流水");
  } catch (error) {
    showToast(error instanceof Error ? error.message : "保存失败");
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
