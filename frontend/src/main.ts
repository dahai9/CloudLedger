import { ClientUpdateRequiredError, cloudLedgerApi } from "./api/cloudLedger";
import { clientVersion } from "./config";
import { createIcons, icons } from "lucide";
import { offlineStore, type OfflineSnapshot } from "./offlineStore";
import "./styles.css";
import type {
  AnalysisMonths,
  ApprovalState,
  AuditPeriod,
  Category,
  Ledger,
  LedgerDashboard,
  FinancialAnalysis,
  FinancialMemberDetail,
  FinancialMonthDetail,
  ClientVersionStatus,
  LoginDraft,
  NewTransactionDraft,
  OfflineTransaction,
  PeriodGranularity,
  TransactionAuditLifecycle,
  TransactionDirection,
  UpdateProfileDraft,
  UserAccount,
  UserSession,
} from "./types";

type ViewMode = "activity" | "analysis" | "approval" | "audit";
type AnalysisTab = "summary" | "month";
type AnalysisDetailTarget =
  | { kind: "month"; month: string }
  | { kind: "member"; memberId: string };
type TransactionFilter = "all" | "pending" | "approved" | "rejected" | "voided";
type AuthStatus = "checking" | "authenticated" | "anonymous";
type SyncPhase = "idle" | "connecting" | "syncing" | "success" | "failed";
type DatePickerScope = "activity" | "audit" | "analysis";

interface DatePickerState {
  scope: DatePickerScope;
  granularity: PeriodGranularity;
  draft: string;
  viewMonth: string;
  opening?: boolean;
  transition?: "previous" | "next";
}

interface SyncState {
  phase: SyncPhase;
  completed: number;
  total: number;
  syncedCount: number;
}

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

interface VoidDialogState {
  transactionId: string;
  reason: string;
}

interface AppState {
  authStatus: AuthStatus;
  userMenuOpen: boolean;
  profileEditing: boolean;
  ledgers: Ledger[];
  dashboard?: LedgerDashboard;
  analysis?: FinancialAnalysis;
  analysisMonths: AnalysisMonths;
  analysisTab: AnalysisTab;
  analysisMonth: string;
  analysisLoading: boolean;
  analysisError?: string;
  analysisDetailTarget?: AnalysisDetailTarget;
  analysisMonthDetail?: FinancialMonthDetail;
  analysisMemberDetail?: FinancialMemberDetail;
  analysisDetailLoading: boolean;
  analysisDetailError?: string;
  analysisMonthLoading: boolean;
  analysisMonthError?: string;
  updateRequired?: ClientVersionStatus;
  cachedDashboards: Record<string, LedgerDashboard>;
  cachedAuditPeriods: Record<string, AuditPeriod>;
  outbox: OfflineTransaction[];
  sync: SyncState;
  reauthRequired: boolean;
  recentlySyncedTransactionIds: Set<string>;
  activeLedgerId?: string;
  currentUser?: UserAccount;
  cloudStatus: UserSession["cloudStatus"];
  loading: boolean;
  pendingAction?: string;
  error?: string;
  view: ViewMode;
  filter: TransactionFilter;
  activityMonth: string;
  activityDay: string;
  activityGranularity: PeriodGranularity;
  auditGranularity: PeriodGranularity;
  auditPeriod: string;
  datePicker?: DatePickerState;
  auditData?: AuditPeriod;
  auditLoading: boolean;
  auditError?: string;
  auditDetailTransactionId?: string;
  voidDialog?: VoidDialogState;
  categoryEditing: boolean;
  categoryName: string;
  amountsVisible: boolean;
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
  analysisTab: "summary",
  analysisMonth: currentMonthKey(),
  analysisLoading: false,
  analysisDetailLoading: false,
  analysisMonthLoading: false,
  cachedDashboards: {},
  cachedAuditPeriods: {},
  outbox: [],
  sync: { phase: "idle", completed: 0, total: 0, syncedCount: 0 },
  reauthRequired: false,
  recentlySyncedTransactionIds: new Set(),
  view: "activity",
  filter: "all",
  activityMonth: currentMonthKey(),
  activityDay: currentDayKey(),
  activityGranularity: "month",
  auditGranularity: "month",
  auditPeriod: currentMonthKey(),
  auditLoading: false,
  categoryEditing: false,
  categoryName: "",
  amountsVisible: false,
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
  timeZone: "Asia/Shanghai",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});
const periodMonthFormatter = new Intl.DateTimeFormat("zh-CN", {
  timeZone: "Asia/Shanghai",
  year: "numeric",
  month: "short",
});
const autoRefreshMs = 10_000;
const pullRefreshThreshold = 88;
const pullRefreshMaximum = 120;
let autoRefreshInFlight = false;
let turnstileScriptPromise: Promise<void> | undefined;
let lastRenderedAuthStatus: AuthStatus | undefined;
let lastRenderedView: ViewMode | undefined;
let syncSettleTimer: number | undefined;
let authoritativeCacheUserId: string | undefined;
let lastSyncPresentationKey: string | undefined;
let pullStartY = 0;
let pullDistance = 0;
let pullProgress = 0;
let pullTracking = false;
let pullArmed = false;
let pullRefreshing = false;

void loadInitialState();
window.setInterval(() => {
  if (shouldAutoRefresh()) {
    void refreshRemoteState({ silent: true });
  }
}, autoRefreshMs);

async function checkClientVersion() {
  if (!navigator.onLine) return true;
  try {
    const status = await cloudLedgerApi.checkClientVersion();
    state.updateRequired = status.updateRequired ? status : undefined;
    return !status.updateRequired;
  } catch (error) {
    if (applyClientUpdate(error)) return false;
    return true;
  }
}

function applyClientUpdate(error: unknown) {
  if (!(error instanceof ClientUpdateRequiredError)) return false;
  state.updateRequired = error.update;
  state.loading = false;
  state.pendingAction = undefined;
  state.datePicker = undefined;
  state.voidDialog = undefined;
  state.toast = undefined;
  state.analysisLoading = false;
  state.analysisDetailLoading = false;
  state.error = undefined;
  resetAnalysisDetail();
  return true;
}

function clientVersionLabel(value: string) {
  return value.startsWith("v") ? value : `v${value}`;
}

window.addEventListener("focus", () => {
  if (state.authStatus === "authenticated") {
    void refreshRemoteState({ silent: true });
  }
});

window.addEventListener("online", () => {
  if (state.authStatus === "authenticated") {
    void refreshRemoteState({ silent: true, announceSync: true });
  }
});

window.addEventListener("offline", () => {
  if (state.authStatus !== "authenticated") return;
  state.cloudStatus = {
    state: "offline",
    label: state.outbox.length > 0 ? `离线 · ${state.outbox.length} 笔未同步` : "离线可记账",
    detail: "正在使用本地账本，恢复网络后会自动同步。",
  };
  state.sync = { phase: "idle", completed: 0, total: state.outbox.length, syncedCount: 0 };
  updateSyncStatusRegion();
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
  if (event.key !== "Escape") {
    return;
  }
  if (state.datePicker) {
    closeDatePicker();
    return;
  }
  if (state.voidDialog) {
    closeVoidDialog();
    return;
  }
  if (state.userMenuOpen) {
    closeUserMenu();
    app.querySelector<HTMLButtonElement>("#userMenuButton")?.focus();
  }
});

document.addEventListener("touchstart", handlePullRefreshStart, { passive: true });
document.addEventListener("touchmove", handlePullRefreshMove, { passive: false });
document.addEventListener("touchend", handlePullRefreshEnd, { passive: true });
document.addEventListener("touchcancel", cancelPullRefresh, { passive: true });

async function loadInitialState() {
  state.loading = true;

  if (!(await checkClientVersion())) {
    state.loading = false;
    render();
    return;
  }

  // A cold start has no in-memory access token. Render the last private
  // snapshot first, then refresh its session and cloud data in the background.
  // After an explicit login, skip this path so another account's cache never
  // flashes before the newly authenticated account loads.
  if (!cloudLedgerApi.getStoredSession() && (await restoreOfflineState())) {
    state.loading = false;
    state.sync = navigator.onLine
      ? { phase: "connecting", completed: 0, total: state.outbox.length, syncedCount: 0 }
      : { phase: "idle", completed: 0, total: state.outbox.length, syncedCount: 0 };
    render();
    void refreshRemoteState({ silent: true, allowPending: true, announceSync: true });
    return;
  }

  render();
  try {
    const session = await cloudLedgerApi.getUserSession();
    await hydrateOfflineCacheForUser(session.currentUser.id);
    const syncedTransactions = await syncOutbox(false);
    const ledgers = await cloudLedgerApi.listLedgers();
    const activeLedgerId = pickDefaultLedgerId(ledgers);

    const dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(
          activeLedgerId,
          state.activityMonth,
          state.activityGranularity === "day" ? state.activityDay : undefined,
        )
      : undefined;
    applyRemoteState(session, ledgers, activeLedgerId, dashboard);
    state.authStatus = "authenticated";
    state.reauthRequired = false;
    if (syncedTransactions.length > 0) {
      state.recentlySyncedTransactionIds = new Set(
        syncedTransactions.map((transaction) => transaction.id),
      );
      state.sync = {
        phase: "success",
        completed: syncedTransactions.length,
        total: syncedTransactions.length,
        syncedCount: syncedTransactions.length,
      };
    } else {
      state.sync = { phase: "idle", completed: 0, total: 0, syncedCount: 0 };
    }
    syncProfileFormFromUser();
    resetFormForDashboard();
    state.error = undefined;
    await saveOfflineSnapshot();
  } catch (error) {
    if (applyClientUpdate(error)) {
      return;
    }
    const restored = await restoreOfflineState();
    if (restored) {
      state.cloudStatus = {
        state: "offline",
        label: state.outbox.length > 0 ? `${state.outbox.length} 笔待同步` : "离线可记账",
        detail: friendlyError(error, "网络不可用"),
      };
      state.sync = { phase: "failed", completed: 0, total: state.outbox.length, syncedCount: 0 };
    } else if (cloudLedgerApi.isAuthRequired(error)) {
      state.authStatus = "anonymous";
      state.currentUser = undefined;
      state.ledgers = [];
      state.activeLedgerId = undefined;
      state.dashboard = undefined;
      state.analysis = undefined;
      state.auditData = undefined;
      state.auditDetailTransactionId = undefined;
      state.cachedDashboards = {};
      state.cachedAuditPeriods = {};
      state.outbox = [];
      state.reauthRequired = false;
      state.sync = { phase: "idle", completed: 0, total: 0, syncedCount: 0 };
      state.userMenuOpen = false;
      state.profileEditing = false;
      state.error = undefined;
    } else {
      state.error = friendlyError(error, "加载失败");
    }
  } finally {
    state.loading = false;
    render();
    if (state.sync.phase === "success") scheduleSyncSettle();
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
    if (applyClientUpdate(error)) return;
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
  const userId = state.currentUser?.id;
  try {
    state.loading = true;
    state.datePicker = undefined;
    state.userMenuOpen = false;
    state.profileEditing = false;
    render();
    await cloudLedgerApi.logout();
  } catch (error) {
    if (applyClientUpdate(error)) return;
    showToast(friendlyError(error, "退出失败"));
  } finally {
    if (userId) {
      await offlineStore.clear(userId).catch(() => undefined);
    }
    state.authStatus = "anonymous";
    state.currentUser = undefined;
    state.ledgers = [];
    state.activeLedgerId = undefined;
    state.dashboard = undefined;
    state.analysis = undefined;
    state.auditData = undefined;
    state.auditDetailTransactionId = undefined;
    resetAnalysisDetail();
    state.cachedDashboards = {};
    state.cachedAuditPeriods = {};
    state.outbox = [];
    state.reauthRequired = false;
    state.recentlySyncedTransactionIds.clear();
    state.sync = { phase: "idle", completed: 0, total: 0, syncedCount: 0 };
    authoritativeCacheUserId = undefined;
    state.loading = false;
    render();
  }
}

async function switchLedger(ledgerId: string) {
  if (state.cloudStatus.state === "offline") {
    const cached = cachedDashboard(ledgerId, currentMonthKey());
    if (!cached) {
      showToast("该账本尚未缓存，联网后可用");
      return;
    }
    state.activeLedgerId = ledgerId;
    state.dashboard = cached;
    state.activityMonth = cached.selectedTransactionMonth;
    state.activityDay = cached.selectedTransactionDay ?? currentDayKey();
    state.auditData = undefined;
    state.auditDetailTransactionId = undefined;
    state.analysis = undefined;
    state.analysisError = undefined;
    state.analysisTab = "summary";
    resetAnalysisMonth();
    resetAnalysisDetail();
    state.view = "activity";
    state.filter = "all";
    resetFormForDashboard();
    render();
    return;
  }
  try {
    state.loading = true;
    state.datePicker = undefined;
    state.activeLedgerId = ledgerId;
    state.activityMonth = currentMonthKey();
    state.activityDay = currentDayKey();
    state.auditPeriod = currentMonthKey();
    state.auditData = undefined;
    state.auditDetailTransactionId = undefined;
    state.categoryEditing = false;
    state.categoryName = "";
    render();
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(ledgerId, state.activityMonth);
    state.activityMonth = state.dashboard.selectedTransactionMonth;
    state.activityDay = state.dashboard.selectedTransactionDay ?? currentDayKey();
    rememberDashboard(state.dashboard);
    state.analysis = undefined;
    state.analysisError = undefined;
    state.analysisTab = "summary";
    resetAnalysisMonth();
    resetAnalysisDetail();
    state.view = "activity";
    state.filter = "all";
    resetFormForDashboard();
    state.error = undefined;
    await saveOfflineSnapshot();
  } catch (error) {
    state.error = friendlyError(error, "切换账本失败");
  } finally {
    state.loading = false;
    render();
  }
}

async function loadActivityMonth(month: string, force = false) {
  const ledgerId = state.activeLedgerId;
  if (!ledgerId || (!force && month === state.activityMonth && !state.dashboard?.selectedTransactionDay) || state.pendingAction) {
    return;
  }
  if (state.cloudStatus.state === "offline") {
    const cached = cachedDashboard(ledgerId, month);
    if (!cached) {
      showToast("该月份尚未缓存，联网后可查看");
      return;
    }
    state.activityMonth = cached.selectedTransactionMonth;
    state.activityDay = cached.selectedTransactionDay ?? currentDayKey();
    state.dashboard = cached;
    resetFormForDashboard({ preserveDraft: true });
    render();
    return;
  }
  try {
    state.pendingAction = "activity-month";
    state.activityMonth = month;
    render();
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(ledgerId, month);
    state.activityMonth = state.dashboard.selectedTransactionMonth;
    state.activityDay = state.dashboard.selectedTransactionDay ?? state.dashboard.availableTransactionDays[0] ?? currentDayKey();
    rememberDashboard(state.dashboard);
    resetFormForDashboard({ preserveDraft: true });
    state.error = undefined;
    await saveOfflineSnapshot();
  } catch (error) {
    showToast(friendlyError(error, "加载月份流水失败"));
  } finally {
    state.pendingAction = undefined;
    render();
  }
}

async function loadActivityDay(day: string, force = false) {
  const ledgerId = state.activeLedgerId;
  if (!ledgerId || (!force && day === state.activityDay && state.dashboard?.selectedTransactionDay === day) || state.pendingAction) return;
  const month = day.slice(0, 7);
  if (state.cloudStatus.state === "offline") {
    const cached = cachedDashboard(ledgerId, month, day);
    if (!cached) {
      showToast("该日期尚未缓存，联网后可查看");
      return;
    }
    state.activityMonth = month;
    state.activityDay = day;
    state.dashboard = cached;
    resetFormForDashboard({ preserveDraft: true });
    render();
    return;
  }
  try {
    state.pendingAction = "activity-day";
    state.activityMonth = month;
    state.activityDay = day;
    render();
    state.dashboard = await cloudLedgerApi.getLedgerDashboard(ledgerId, month, day);
    state.activityMonth = state.dashboard.selectedTransactionMonth;
    state.activityDay = state.dashboard.selectedTransactionDay ?? day;
    rememberDashboard(state.dashboard);
    resetFormForDashboard({ preserveDraft: true });
    state.error = undefined;
    await saveOfflineSnapshot();
  } catch (error) {
    showToast(friendlyError(error, "加载日期流水失败"));
  } finally {
    state.pendingAction = undefined;
    render();
  }
}

async function changeActivityGranularity(granularity: PeriodGranularity) {
  if (state.activityGranularity === granularity || state.pendingAction) return;
  state.activityGranularity = granularity;
  if (granularity === "month") {
    await loadActivityMonth(state.activityMonth, true);
    return;
  }
  const day =
    state.dashboard?.availableTransactionDays.find((candidate) => candidate === state.activityDay) ??
    state.dashboard?.availableTransactionDays[0] ??
    (state.activityMonth === currentMonthKey() ? currentDayKey() : `${state.activityMonth}-01`);
  state.activityDay = day;
  await loadActivityDay(day, true);
}

function openDatePicker(scope: DatePickerScope, granularity: PeriodGranularity) {
  const value =
    scope === "activity"
      ? granularity === "day"
        ? state.activityDay
        : state.activityMonth
      : scope === "audit"
        ? state.auditPeriod
        : state.analysisMonth;
  const draft = value || (granularity === "day" ? currentDayKey() : currentMonthKey());
  state.datePicker = {
    scope,
    granularity,
    draft,
    viewMonth: granularity === "day" ? draft.slice(0, 7) : draft,
    opening: true,
  };
  render();
  const openingPicker = state.datePicker;
  window.requestAnimationFrame(() => {
    if (state.datePicker === openingPicker) {
      state.datePicker = { ...state.datePicker, opening: undefined };
    }
    app.querySelector<HTMLButtonElement>("#datePickerDialog [data-picker-close]")?.focus();
  });
}

function closeDatePicker() {
  if (!state.datePicker) return;
  state.datePicker = undefined;
  render();
}

function updateDatePickerDraft(value: string) {
  const picker = state.datePicker;
  if (!picker) return;
  state.datePicker = {
    ...picker,
    draft: value,
    viewMonth: picker.granularity === "day" ? value.slice(0, 7) : value,
    transition: undefined,
  };
  render();
}

function navigateDatePicker(direction: "previous" | "next") {
  const picker = state.datePicker;
  if (!picker) return;
  const delta = direction === "previous" ? -1 : 1;
  state.datePicker = {
    ...picker,
    viewMonth: shiftMonthKey(picker.viewMonth, picker.granularity === "month" ? delta * 12 : delta),
    transition: picker.granularity === "day" ? direction : undefined,
  };
  render();
}

function setDatePickerToday() {
  const picker = state.datePicker;
  if (!picker) return;
  const value = picker.granularity === "day" ? currentDayKey() : currentMonthKey();
  updateDatePickerDraft(value);
}

function confirmDatePicker() {
  const picker = state.datePicker;
  if (!picker) return;
  state.datePicker = undefined;
  render();
  if (picker.scope === "activity") {
    if (picker.granularity === "day") {
      void loadActivityDay(picker.draft);
    } else {
      void loadActivityMonth(picker.draft);
    }
    return;
  }
  if (picker.scope === "analysis") {
    state.analysisTab = "month";
    void loadAnalysisMonth(picker.draft, true);
    return;
  }
  setAuditPeriod(picker.granularity, picker.draft);
}

function renderDatePicker() {
  const picker = state.datePicker;
  if (!picker) return "";
  const title = picker.granularity === "day" ? "选择日期" : "选择月份";
  const selection =
    picker.granularity === "day" ? formatDayLabel(picker.draft) : formatMonthLabel(picker.draft);
  const context = picker.scope === "activity" ? "流水" : picker.scope === "audit" ? "审计" : "分析";
  const availableMonths =
    picker.scope === "activity" ? state.dashboard?.availableTransactionMonths ?? [] : [];
  const helper =
    picker.granularity === "day"
      ? picker.scope === "activity"
        ? `${state.dashboard?.availableTransactionDays.length ?? 0} 个有流水日期`
        : "生命周期记录"
      : picker.scope === "activity"
        ? `${availableMonths.length} 个可查看月份`
        : "审计记录";
  const transitionClass = picker.transition ? ` is-transitioning-${picker.transition}` : "";
  const openingClass = picker.opening ? " is-entering" : "";

  return `
    <div class="date-picker-backdrop${openingClass}" data-picker-backdrop>
      <section class="date-picker-dialog${openingClass}" id="datePickerDialog" role="dialog" aria-modal="true" aria-labelledby="datePickerTitle">
        <header class="date-picker-header">
          <div class="date-picker-title-wrap">
            <span class="date-picker-icon" aria-hidden="true"><i data-lucide="${picker.granularity === "day" ? "calendar-days" : "calendar-range"}"></i></span>
            <div>
              <span class="date-picker-context">${context} · 时间范围</span>
              <h2 id="datePickerTitle">${title}</h2>
            </div>
          </div>
          <button class="date-picker-close" type="button" data-picker-close aria-label="关闭日期选择器" title="关闭"><i data-lucide="x"></i></button>
        </header>
        <div class="date-picker-selection">
          <span>当前选择</span>
          <strong>${escapeHtml(selection)}</strong>
          <small>${escapeHtml(helper)}</small>
        </div>
        <div class="date-picker-content${transitionClass}">
          ${picker.granularity === "day" ? renderDatePickerDayGrid(picker) : renderDatePickerMonthGrid(picker)}
        </div>
        <footer class="date-picker-footer">
          <button class="date-picker-secondary" type="button" data-picker-today>${picker.granularity === "day" ? "今天" : "本月"}</button>
          <div class="date-picker-footer-actions">
            <button class="date-picker-secondary" type="button" data-picker-close>取消</button>
            <button class="date-picker-primary" type="button" data-picker-confirm>确定</button>
          </div>
        </footer>
      </section>
    </div>
  `;
}

function renderDatePickerMonthGrid(picker: DatePickerState) {
  const year = Number(picker.viewMonth.slice(0, 4));
  const available = new Set(
    picker.scope === "activity" ? state.dashboard?.availableTransactionMonths ?? [] : [],
  );
  const months = Array.from({ length: 12 }, (_, index) => {
    const month = `${year}-${String(index + 1).padStart(2, "0")}`;
    const disabled =
      (picker.scope === "activity" && !available.has(month) && picker.draft !== month) ||
      (picker.scope === "analysis" && month > currentMonthKey());
    const active = picker.draft === month;
    return `
      <button class="date-picker-month ${active ? "is-selected" : ""}" type="button" data-picker-value="${month}" aria-pressed="${active}" ${disabled ? "disabled" : ""}>
        <strong>${index + 1}月</strong>
        <span>${disabled ? (picker.scope === "analysis" ? "尚未到达" : "暂无流水") : picker.scope === "activity" ? "可查看" : picker.scope === "analysis" ? "可分析" : "审计记录"}</span>
      </button>
    `;
  }).join("");
  return `
    <div class="date-picker-period-heading">
      <button class="date-picker-nav" type="button" data-picker-nav="previous" aria-label="上一年" title="上一年"><i data-lucide="chevron-left"></i></button>
      <strong>${year}年</strong>
      <button class="date-picker-nav" type="button" data-picker-nav="next" aria-label="下一年" title="下一年"><i data-lucide="chevron-right"></i></button>
    </div>
    <div class="date-picker-month-grid" role="grid" aria-label="${year}年月份">
      ${months}
    </div>
  `;
}

function renderDatePickerDayGrid(picker: DatePickerState) {
  const month = picker.viewMonth;
  const year = Number(month.slice(0, 4));
  const monthNumber = Number(month.slice(5, 7));
  const firstWeekday = new Date(Date.UTC(year, monthNumber - 1, 1)).getUTCDay();
  const dayCount = daysInMonthKey(month);
  const available =
    picker.scope === "activity" && state.dashboard?.selectedTransactionMonth === month
      ? new Set(state.dashboard.availableTransactionDays)
      : undefined;
  const cells = Array.from({ length: firstWeekday }, () => '<span class="date-picker-day is-empty" aria-hidden="true"></span>');
  for (let day = 1; day <= dayCount; day += 1) {
    const value = `${month}-${String(day).padStart(2, "0")}`;
    const active = picker.draft === value;
    const today = currentDayKey() === value;
    const hasData = available?.has(value) ?? false;
    cells.push(`
      <button class="date-picker-day ${active ? "is-selected" : ""} ${today ? "is-today" : ""}" type="button" data-picker-value="${value}" aria-label="${formatDayLabel(value)}" aria-pressed="${active}">
        <span>${day}</span>${hasData ? '<i class="date-picker-day-dot" aria-label="有流水"></i>' : ""}
      </button>
    `);
  }
  return `
    <div class="date-picker-period-heading">
      <button class="date-picker-nav" type="button" data-picker-nav="previous" aria-label="上个月" title="上个月"><i data-lucide="chevron-left"></i></button>
      <strong>${escapeHtml(formatMonthLabel(month))}</strong>
      <button class="date-picker-nav" type="button" data-picker-nav="next" aria-label="下个月" title="下个月"><i data-lucide="chevron-right"></i></button>
    </div>
    <div class="date-picker-weekdays" aria-hidden="true">
      ${["日", "一", "二", "三", "四", "五", "六"].map((day) => `<span>${day}</span>`).join("")}
    </div>
    <div class="date-picker-day-grid" role="grid" aria-label="${escapeHtml(formatMonthLabel(month))}">
      ${cells.join("")}
    </div>
  `;
}

function renderVoidDialog() {
  const dialog = state.voidDialog;
  if (!dialog) return "";
  const transaction = state.dashboard?.recentTransactions.find(
    (item) => item.id === dialog.transactionId,
  );
  if (!transaction) return "";
  const processing = state.pendingAction === transaction.id;
  const reason = escapeHtml(dialog.reason);
  return `
    <div class="void-backdrop" data-void-backdrop>
      <section class="void-dialog" role="dialog" aria-modal="true" aria-labelledby="voidDialogTitle">
        <header class="void-dialog-header">
          <div class="void-dialog-title-wrap">
            <span class="void-dialog-icon" aria-hidden="true"><i data-lucide="ban"></i></span>
            <div>
              <span class="void-dialog-kicker">流水更正</span>
              <h2 id="voidDialogTitle">作废这笔流水</h2>
            </div>
          </div>
          <button class="void-dialog-close" type="button" data-void-cancel aria-label="关闭作废窗口" title="关闭"><i data-lucide="x"></i></button>
        </header>
        <div class="void-dialog-body">
          <div class="void-transaction-summary">
            <div>
              <strong>${escapeHtml(transaction.title)}</strong>
              <span>${escapeHtml(transaction.accountName)} · ${escapeHtml(transaction.categoryName)} · ${formatDate(transaction.occurredAt)}</span>
            </div>
            <strong class="${transaction.direction === "expense" ? "amount-out" : "amount-in"}">${formatSignedMoney(transaction.amountCents, state.dashboard?.ledger.currency ?? "CNY", transaction.direction)}</strong>
          </div>
          <p class="void-warning">作废后不再计入余额和财务分析，但不会自动追回已经发生的真实款项。原流水和审计记录会继续保留。</p>
          <label class="void-reason-field">
            <span>作废原因</span>
            <textarea id="voidReasonInput" maxlength="200" rows="4" placeholder="请输入作废原因">${reason}</textarea>
            <small>必填，最多 200 个字符</small>
          </label>
        </div>
        <footer class="void-dialog-footer">
          <button class="secondary-button" type="button" data-void-cancel ${processing ? "disabled" : ""}>取消</button>
          <button class="danger-button" type="button" data-void-confirm ${processing || !dialog.reason.trim() ? "disabled" : ""}>${processing ? "处理中" : "确认作废"}</button>
        </footer>
      </section>
    </div>
  `;
}

async function refreshDashboard() {
  await refreshRemoteState({ silent: false, allowPending: true });
}

async function changeView(view: ViewMode) {
  state.datePicker = undefined;
  state.view = view;
  if (view !== "audit") {
    state.auditDetailTransactionId = undefined;
  }
  render();
  if (view === "analysis") {
    await loadFinancialAnalysis(false);
  } else if (view === "audit") {
    await loadAuditPeriod(false);
  }
}

async function loadAuditPeriod(force = false) {
  const dashboard = state.dashboard;
  if (!dashboard || state.auditLoading) return;
  if (
    !force &&
    state.auditData?.ledgerId === dashboard.ledger.id &&
    state.auditData.granularity === state.auditGranularity &&
    state.auditData.period === state.auditPeriod
  ) {
    return;
  }
  const ledgerId = dashboard.ledger.id;
  const granularity = state.auditGranularity;
  const period = state.auditPeriod;
  const cacheKey = auditPeriodCacheKey(ledgerId, granularity, period);
  if (state.cloudStatus.state === "offline") {
    const cached = state.cachedAuditPeriods[cacheKey];
    if (cached) {
      state.auditData = cached;
      state.auditError = undefined;
    } else {
      state.auditError = "该日期或月份尚未缓存，联网后可查看";
    }
    render();
    return;
  }
  state.auditLoading = true;
  state.auditError = undefined;
  render();
  try {
    const data = await cloudLedgerApi.getAuditPeriod(ledgerId, granularity, period);
    if (
      state.activeLedgerId === ledgerId &&
      state.auditGranularity === granularity &&
      state.auditPeriod === period
    ) {
      state.auditData = data;
      state.cachedAuditPeriods[cacheKey] = data;
      await saveOfflineSnapshot();
    }
  } catch (error) {
    if (state.activeLedgerId === ledgerId && state.auditPeriod === period) {
      state.auditError = friendlyError(error, "加载审计生命周期失败");
    }
  } finally {
    state.auditLoading = false;
    render();
  }
}

function setAuditPeriod(granularity: PeriodGranularity, period: string) {
  state.auditGranularity = granularity;
  state.auditPeriod = period;
  state.auditData = undefined;
  state.auditDetailTransactionId = undefined;
  void loadAuditPeriod(true);
}

function auditPeriodCacheKey(
  ledgerId: string,
  granularity: PeriodGranularity,
  period: string,
) {
  return `${ledgerId}:${granularity}:${period}`;
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
    if (applyClientUpdate(error)) return;
    if (state.activeLedgerId === ledgerId && state.analysisMonths === months) {
      state.analysisError = friendlyError(error, "加载财务分析失败");
    }
  } finally {
    state.analysisLoading = false;
    render();
  }
}

async function loadAnalysisMonth(month: string, force = false) {
  const dashboard = state.dashboard;
  if (
    !dashboard ||
    dashboard.ledger.role !== "business_owner" ||
    dashboard.ledger.kind !== "organization" ||
    state.analysisMonthLoading
  ) {
    return;
  }
  if (!force && state.analysisMonth === month && state.analysisMonthDetail?.month === month) {
    return;
  }
  const ledgerId = dashboard.ledger.id;
  state.analysisMonth = month;
  state.analysisMonthDetail = undefined;
  state.analysisMonthError = undefined;
  state.analysisMonthLoading = true;
  render();
  try {
    const detail = await cloudLedgerApi.getFinancialMonthDetail(ledgerId, month);
    if (state.activeLedgerId === ledgerId && state.analysisMonth === month) {
      state.analysisMonthDetail = detail;
    }
  } catch (error) {
    if (applyClientUpdate(error)) return;
    if (state.activeLedgerId === ledgerId && state.analysisMonth === month) {
      state.analysisMonthError = friendlyError(error, "加载月度分析失败");
    }
  } finally {
    state.analysisMonthLoading = false;
    render();
  }
}

async function loadAnalysisDetail(target: AnalysisDetailTarget, force = false) {
  const dashboard = state.dashboard;
  if (
    !dashboard ||
    dashboard.ledger.kind !== "organization" ||
    dashboard.ledger.role !== "business_owner" ||
    state.analysisDetailLoading
  ) {
    return;
  }
  const sameTarget = analysisTargetsEqual(state.analysisDetailTarget, target);
  if (!force && sameTarget && (state.analysisMonthDetail || state.analysisMemberDetail)) return;

  state.analysisDetailTarget = target;
  state.analysisMonthDetail = undefined;
  state.analysisMemberDetail = undefined;
  state.analysisDetailError = undefined;
  state.analysisDetailLoading = true;
  render();
  const ledgerId = dashboard.ledger.id;
  const months = state.analysisMonths;
  try {
    if (target.kind === "month") {
      const detail = await cloudLedgerApi.getFinancialMonthDetail(ledgerId, target.month);
      if (state.activeLedgerId === ledgerId && analysisTargetsEqual(state.analysisDetailTarget, target)) {
        state.analysisMonthDetail = detail;
      }
    } else {
      const detail = await cloudLedgerApi.getFinancialMemberDetail(
        ledgerId,
        months,
        target.memberId,
      );
      if (
        state.activeLedgerId === ledgerId &&
        state.analysisMonths === months &&
        analysisTargetsEqual(state.analysisDetailTarget, target)
      ) {
        state.analysisMemberDetail = detail;
      }
    }
  } catch (error) {
    if (applyClientUpdate(error)) return;
    if (state.activeLedgerId === ledgerId && analysisTargetsEqual(state.analysisDetailTarget, target)) {
      state.analysisDetailError = friendlyError(error, "加载分析详情失败");
    }
  } finally {
    state.analysisDetailLoading = false;
    render();
  }
}

function closeAnalysisDetail() {
  resetAnalysisDetail();
  render();
}

function resetAnalysisDetail() {
  state.analysisDetailTarget = undefined;
  state.analysisMonthDetail = undefined;
  state.analysisMemberDetail = undefined;
  state.analysisDetailError = undefined;
  state.analysisDetailLoading = false;
}

function resetAnalysisMonth() {
  state.analysisMonth = currentMonthKey();
  state.analysisMonthDetail = undefined;
  state.analysisMonthError = undefined;
  state.analysisMonthLoading = false;
}

function analysisTargetsEqual(
  left: AnalysisDetailTarget | undefined,
  right: AnalysisDetailTarget | undefined,
) {
  if (!left || !right || left.kind !== right.kind) return left === right;
  return left.kind === "month"
    ? left.month === (right as Extract<AnalysisDetailTarget, { kind: "month" }>).month
    : left.memberId === (right as Extract<AnalysisDetailTarget, { kind: "member" }>).memberId;
}

async function refreshRemoteState(
  options: { silent: boolean; allowPending?: boolean; announceSync?: boolean } = { silent: true },
) {
  if (
    !state.activeLedgerId ||
    state.updateRequired ||
    autoRefreshInFlight ||
    (state.pendingAction && !options.allowPending)
  ) {
    return;
  }

  if (!(await checkClientVersion())) return;

  const before = visibleStateFingerprint();
  const expectedUserId = state.currentUser?.id;
  const previousLedgerId = state.activeLedgerId;
  const announceSync = options.announceSync === true;
  autoRefreshInFlight = true;
  if (announceSync) {
    state.cloudStatus = { state: "checking", label: "正在连接云端" };
    state.sync = {
      phase: "connecting",
      completed: 0,
      total: state.outbox.length,
      syncedCount: 0,
    };
    updateSyncStatusRegion();
  }

  try {
    const session = await cloudLedgerApi.getUserSession();
    if (expectedUserId && session.currentUser.id !== expectedUserId) {
      state.reauthRequired = true;
      state.cloudStatus = { state: "offline", label: "登录账号不匹配" };
      state.sync = { phase: "failed", completed: 0, total: state.outbox.length, syncedCount: 0 };
      return;
    }

    await hydrateOfflineCacheForUser(session.currentUser.id);
    const syncedTransactions = await syncOutbox(announceSync || state.outbox.length > 0);
    const ledgers = await cloudLedgerApi.listLedgers();
    const currentLedgerId = state.activeLedgerId;
    const activeLedgerId = ledgers.some((ledger) => ledger.id === currentLedgerId)
      ? currentLedgerId
      : ledgers.some((ledger) => ledger.id === previousLedgerId)
        ? previousLedgerId
      : pickDefaultLedgerId(ledgers);

    const dashboard = activeLedgerId
      ? await cloudLedgerApi.getLedgerDashboard(
          activeLedgerId,
          state.activityMonth,
          state.activityGranularity === "day" ? state.activityDay : undefined,
        )
      : undefined;
    if (expectedUserId && state.currentUser?.id !== expectedUserId) return;
    applyRemoteState(session, ledgers, activeLedgerId, dashboard);
    state.reauthRequired = false;
    state.error = undefined;
    syncProfileFormFromUser();
    resetFormForDashboard({ preserveDraft: true });
    if (syncedTransactions.length > 0) {
      state.recentlySyncedTransactionIds = new Set(
        syncedTransactions.map((transaction) => transaction.id),
      );
      state.sync = {
        phase: "success",
        completed: syncedTransactions.length,
        total: syncedTransactions.length,
        syncedCount: syncedTransactions.length,
      };
    } else {
      state.sync = { phase: "idle", completed: 0, total: 0, syncedCount: 0 };
    }
    await saveOfflineSnapshot();
  } catch (error) {
    if (applyClientUpdate(error)) {
      return;
    }
    if (cloudLedgerApi.isAuthRequired(error)) {
      if (state.dashboard && state.currentUser) {
        state.reauthRequired = true;
        state.cloudStatus = { state: "offline", label: "登录已过期" };
        state.sync = {
          phase: "failed",
          completed: 0,
          total: state.outbox.length,
          syncedCount: 0,
        };
      } else {
        state.authStatus = "anonymous";
        state.currentUser = undefined;
        state.ledgers = [];
        state.activeLedgerId = undefined;
        state.dashboard = undefined;
        state.analysis = undefined;
        state.auditData = undefined;
        state.auditDetailTransactionId = undefined;
        state.cachedDashboards = {};
        state.cachedAuditPeriods = {};
        state.outbox = [];
        state.userMenuOpen = false;
        state.profileEditing = false;
        state.error = undefined;
      }
    } else {
      state.cloudStatus = {
        state: "offline",
        label: state.outbox.length > 0 ? `${state.outbox.length} 笔待同步` : "离线可记账",
        detail: friendlyError(error, "网络不可用"),
      };
      state.sync = {
        phase: announceSync ? "failed" : "idle",
        completed: 0,
        total: state.outbox.length,
        syncedCount: 0,
      };
      updateCloudStatusLabel();
      if (!options.silent) {
        state.error = friendlyError(error, "刷新失败");
      }
    }
  } finally {
    autoRefreshInFlight = false;
    if (before !== visibleStateFingerprint()) {
      render();
    } else {
      updateSyncStatusRegion();
    }
    if (state.sync.phase === "success") scheduleSyncSettle();
  }
}

function applyRemoteState(
  session: UserSession,
  ledgers: Ledger[],
  activeLedgerId: string | undefined,
  dashboard: LedgerDashboard | undefined,
) {
  state.currentUser = session.currentUser;
  state.cloudStatus = session.cloudStatus;
  state.ledgers = ledgers;
  state.activeLedgerId = activeLedgerId;
  state.dashboard = dashboard;
  state.activityMonth = dashboard?.selectedTransactionMonth ?? currentMonthKey();
  state.activityDay = dashboard?.selectedTransactionDay ?? currentDayKey();
  if (state.auditData && state.auditData.ledgerId !== activeLedgerId) {
    state.auditData = undefined;
    state.auditDetailTransactionId = undefined;
  }
  if (dashboard) rememberDashboard(dashboard);
  updateCloudStatusLabel();
}

function dashboardCacheKey(ledgerId: string, month: string, day?: string) {
  return `${ledgerId}:${month}:${day ?? "month"}`;
}

function rememberDashboard(dashboard: LedgerDashboard) {
  state.cachedDashboards[
    dashboardCacheKey(
      dashboard.ledger.id,
      dashboard.selectedTransactionMonth,
      dashboard.selectedTransactionDay,
    )
  ] =
    dashboard;
}

function cachedDashboard(ledgerId: string, month: string, day?: string) {
  return (
    state.cachedDashboards[dashboardCacheKey(ledgerId, month, day)] ??
    (!day ? state.cachedDashboards[dashboardCacheKey(ledgerId, month)] : undefined) ??
    Object.values(state.cachedDashboards).find(
      (dashboard) =>
        dashboard.ledger.id === ledgerId &&
        dashboard.selectedTransactionMonth === month &&
        (day ? dashboard.selectedTransactionDay === day : !dashboard.selectedTransactionDay),
    )
  );
}

async function restoreOfflineState(): Promise<boolean> {
  const snapshot = await offlineStore.loadLast().catch(() => undefined);
  if (!snapshot || !snapshot.ledgers.length) {
    return false;
  }
  const activeLedgerId = snapshot.activeLedgerId ?? pickDefaultLedgerId(snapshot.ledgers);
  const dashboard = activeLedgerId
    ? snapshot.dashboards[dashboardCacheKey(activeLedgerId, snapshot.activityMonth)] ??
      Object.values(snapshot.dashboards).find(
        (item) =>
          item.ledger.id === activeLedgerId &&
          item.selectedTransactionMonth === snapshot.activityMonth &&
          !item.selectedTransactionDay,
      )
    : undefined;
  if (!dashboard) {
    return false;
  }

  state.currentUser = snapshot.user;
  state.ledgers = snapshot.ledgers;
  state.cachedDashboards = snapshot.dashboards;
  state.cachedAuditPeriods = snapshot.auditPeriods;
  state.outbox = snapshot.outbox;
  state.activeLedgerId = activeLedgerId;
  state.dashboard = dashboard;
  state.activityMonth = dashboard.selectedTransactionMonth;
  state.activityDay = dashboard.selectedTransactionDay ?? currentDayKey();
  state.analysis = undefined;
  state.analysisError = undefined;
  state.analysisTab = "summary";
  resetAnalysisMonth();
  state.authStatus = "authenticated";
  state.error = undefined;
  state.cloudStatus = navigator.onLine
    ? { state: "checking", label: "正在连接云端" }
    : {
        state: "offline",
        label: "离线可记账",
        detail: "正在使用本地账本，恢复网络后会自动同步。",
      };
  state.reauthRequired = false;
  updateCloudStatusLabel();
  syncProfileFormFromUser();
  resetFormForDashboard({ preserveDraft: true });
  return true;
}

async function hydrateOfflineCacheForUser(userId: string) {
  if (authoritativeCacheUserId === userId) return;
  const snapshot = await offlineStore.loadAuthoritative().catch(() => undefined);
  authoritativeCacheUserId = userId;
  if (!snapshot || snapshot.user.id !== userId) {
    state.cachedDashboards = {};
    state.cachedAuditPeriods = {};
    state.outbox = [];
    return;
  }
  state.cachedDashboards = snapshot.dashboards;
  state.cachedAuditPeriods = snapshot.auditPeriods;
  state.outbox = snapshot.outbox;
  const dashboard = state.activeLedgerId
    ? cachedDashboard(state.activeLedgerId, state.activityMonth)
    : undefined;
  if (dashboard) state.dashboard = dashboard;
}

function offlineSnapshot(): OfflineSnapshot | undefined {
  if (!state.currentUser || !state.dashboard) {
    return undefined;
  }
  return {
    version: 3,
    user: state.currentUser,
    ledgers: state.ledgers,
    dashboards: state.cachedDashboards,
    auditPeriods: state.cachedAuditPeriods,
    activeLedgerId: state.activeLedgerId,
    activityMonth: state.activityMonth,
    outbox: state.outbox,
  };
}

async function saveOfflineSnapshot() {
  const snapshot = offlineSnapshot();
  if (snapshot) {
    await offlineStore.save(snapshot);
  }
}

async function syncOutbox(announce: boolean) {
  const queue = [...state.outbox];
  const synced: LedgerDashboard["recentTransactions"] = [];
  if (queue.length === 0) return synced;

  state.sync = { phase: "syncing", completed: 0, total: queue.length, syncedCount: 0 };
  if (announce) updateSyncStatusRegion();

  for (const queued of queue) {
    updateTransactionSyncBadge(queued.localId, "syncing");
    const transaction = await cloudLedgerApi.createTransaction(
      queued.draft,
      queued.clientMutationId,
    );
    replaceLocalTransaction(queued.localId, transaction);
    state.outbox = state.outbox.filter(
      (item) => item.clientMutationId !== queued.clientMutationId,
    );
    synced.push(transaction);
    state.sync.completed = synced.length;
    updateCloudStatusLabel();
    await saveOfflineSnapshot();
    updateTransactionSyncBadge(queued.localId, "synced");
    if (announce) updateSyncStatusRegion();
  }
  return synced;
}

function replaceLocalTransaction(
  localId: string,
  transaction: LedgerDashboard["recentTransactions"][number],
) {
  for (const [key, dashboard] of Object.entries(state.cachedDashboards)) {
    if (!dashboard.recentTransactions.some((item) => item.id === localId)) continue;
    state.cachedDashboards[key] = {
      ...dashboard,
      recentTransactions: dashboard.recentTransactions.map((item) =>
        item.id === localId ? transaction : item,
      ),
    };
  }
  if (state.dashboard?.recentTransactions.some((item) => item.id === localId)) {
    state.dashboard = {
      ...state.dashboard,
      recentTransactions: state.dashboard.recentTransactions.map((item) =>
        item.id === localId ? transaction : item,
      ),
    };
    rememberDashboard(state.dashboard);
  }
}

function updateCloudStatusLabel() {
  const pending = state.outbox.length;
  if (state.cloudStatus.state === "offline") {
    state.cloudStatus = {
      ...state.cloudStatus,
      label: pending > 0 ? `离线 · ${pending} 笔未同步` : "离线可记账",
      detail: "正在使用本地账本，恢复网络后会自动同步。",
    };
  } else if (pending > 0) {
    state.cloudStatus = {
      ...state.cloudStatus,
      label: `云端在线 · ${pending} 笔未同步`,
    };
  }
}

function visibleStateFingerprint() {
  return JSON.stringify({
    authStatus: state.authStatus,
    currentUser: state.currentUser,
    ledgers: state.ledgers,
    dashboard: state.dashboard,
    activeLedgerId: state.activeLedgerId,
    activityMonth: state.activityMonth,
    outbox: state.outbox.map((item) => item.clientMutationId),
    error: state.error,
    reauthRequired: state.reauthRequired,
  });
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
  const updateRequired = state.updateRequired !== undefined;
  const sceneIsEntering =
    lastRenderedAuthStatus === undefined ||
    lastRenderedAuthStatus !== state.authStatus ||
    (state.authStatus === "authenticated" && lastRenderedView !== state.view);
  const sceneMotionClass = sceneIsEntering ? "is-entering" : "";

  app.innerHTML = `
    <main
      class="app-shell"
      data-auth-state="${escapeHtml(state.authStatus)}"
      data-current-user-id="${escapeHtml(state.currentUser?.id ?? "")}"
      data-active-ledger-id="${escapeHtml(state.activeLedgerId ?? "")}"
      data-cloud-state="${escapeHtml(state.cloudStatus.state)}"
      data-active-view="${escapeHtml(state.view)}"
      data-amount-visibility="${state.amountsVisible ? "visible" : "hidden"}"
    >
      ${!updateRequired && state.authStatus === "authenticated" && dashboard ? renderPullRefreshIndicator() : ""}
      ${!updateRequired && state.authStatus === "authenticated" ? renderTopBar() : ""}
      ${
        state.updateRequired
          ? renderClientUpdateRequired(state.updateRequired)
          : state.authStatus === "anonymous"
          ? renderLogin(sceneMotionClass)
          : state.loading && !dashboard
          ? renderLoading(sceneMotionClass)
          : dashboard && ledger
            ? renderDashboard(dashboard, sceneMotionClass)
            : state.error
              ? renderError(sceneMotionClass)
              : renderEmptyState(sceneMotionClass)
      }
      ${!updateRequired && dashboard && ledger ? renderBottomNav(dashboard) : ""}
      ${!updateRequired && state.toast ? `<div class="toast" role="status">${escapeHtml(state.toast)}</div>` : ""}
      ${!updateRequired ? renderDatePicker() : ""}
      ${!updateRequired ? renderVoidDialog() : ""}
    </main>
  `;

  bindEvents();
  createIcons({
    icons,
    attrs: { width: "18", height: "18", "stroke-width": "2" },
  });
  if (state.authStatus === "anonymous" && state.authForm.turnstileSiteKey) {
    void mountTurnstile().catch(() => {
      turnstileScriptPromise = undefined;
      state.authForm.turnstileToken = "";
      state.authForm.turnstileSiteKey = undefined;
      showToast("人机验证服务暂时不可用");
    });
  }
  lastSyncPresentationKey = JSON.stringify(syncPresentation());
  lastRenderedAuthStatus = state.authStatus;
  lastRenderedView = state.view;
}

function renderClientUpdateRequired(update: ClientVersionStatus) {
  return `
    <section class="state-panel update-required-panel" data-update-required>
      <div class="update-required-icon" aria-hidden="true"><i data-lucide="download"></i></div>
      <span class="section-kicker">Update required</span>
      <h1>应用版本已过期</h1>
      <p>当前版本 ${escapeHtml(clientVersionLabel(clientVersion))} 已停止支持，请下载最新版本后继续使用。</p>
      <p class="update-required-meta">最低支持版本：${escapeHtml(clientVersionLabel(update.minSupportedVersion))}</p>
      <button class="primary-button" id="openUpdateDownloadButton" type="button">下载新版本</button>
      <small>将跳转到官方下载页面</small>
    </section>
  `;
}

function renderPullRefreshIndicator() {
  const visible = pullTracking || pullRefreshing;
  const classNames = [
    "pull-refresh-indicator",
    visible ? "is-visible" : "",
    pullTracking ? "is-pulling" : "",
    pullArmed ? "is-armed" : "",
    pullRefreshing ? "is-refreshing" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return `<div
    class="${classNames}"
    id="pullRefreshIndicator"
    role="status"
    aria-live="polite"
    aria-label="${pullRefreshing ? "正在刷新账本" : ""}"
    ${visible ? "" : 'aria-hidden="true"'}
    style="--pull-angle:${Math.round(pullProgress * 360)}deg;--pull-offset:${Math.round(pullDistance)}px;--pull-rotation:${Math.round(pullProgress * 210)}deg;--pull-scale:${(0.86 + pullProgress * 0.14).toFixed(3)}"
  >
    <span class="pull-refresh-ring" aria-hidden="true">
      <i class="pull-refresh-spinner" data-lucide="refresh-cw"></i>
      <i class="pull-refresh-check" data-lucide="check"></i>
    </span>
  </div>`;
}

function handlePullRefreshStart(event: TouchEvent) {
  if (
    event.touches.length !== 1 ||
    pullRefreshing ||
    autoRefreshInFlight ||
    Boolean(state.pendingAction) ||
    state.authStatus !== "authenticated" ||
    !state.dashboard ||
    window.scrollY > 0 ||
    isInteractivePullTarget(event.target)
  ) {
    return;
  }
  pullStartY = event.touches[0].clientY;
  pullDistance = 0;
  pullProgress = 0;
  pullTracking = true;
  pullArmed = false;
}

function handlePullRefreshMove(event: TouchEvent) {
  if (!pullTracking || event.touches.length !== 1) return;
  if (window.scrollY > 0) {
    cancelPullRefresh();
    return;
  }

  const rawDistance = event.touches[0].clientY - pullStartY;
  if (rawDistance <= 0) {
    updatePullRefreshIndicator(0);
    return;
  }

  event.preventDefault();
  const resistedDistance =
    rawDistance <= pullRefreshThreshold
      ? rawDistance
      : pullRefreshThreshold + (rawDistance - pullRefreshThreshold) * 0.24;
  pullProgress = Math.min(rawDistance / pullRefreshThreshold, 1);
  pullDistance = Math.min(resistedDistance, pullRefreshMaximum);
  pullArmed = pullProgress >= 1;
  updatePullRefreshIndicator();
}

function handlePullRefreshEnd() {
  if (!pullTracking) return;
  pullTracking = false;
  if (pullArmed) {
    void triggerPullRefresh();
  } else {
    cancelPullRefresh();
  }
}

function cancelPullRefresh() {
  if (pullRefreshing) return;
  pullTracking = false;
  pullArmed = false;
  pullProgress = 0;
  pullDistance = 0;
  updatePullRefreshIndicator();
}

function updatePullRefreshIndicator(forcedProgress?: number) {
  if (forcedProgress !== undefined) {
    pullProgress = forcedProgress;
    pullDistance = 0;
    pullArmed = false;
  }
  const current = app.querySelector<HTMLElement>("#pullRefreshIndicator");
  if (!current) return;
  current.classList.toggle("is-visible", pullTracking || pullRefreshing);
  current.classList.toggle("is-pulling", pullTracking);
  current.classList.toggle("is-armed", pullArmed);
  current.classList.toggle("is-refreshing", pullRefreshing);
  current.style.setProperty("--pull-angle", `${Math.round(pullProgress * 360)}deg`);
  current.style.setProperty("--pull-offset", `${Math.round(pullDistance)}px`);
  current.style.setProperty("--pull-rotation", `${Math.round(pullProgress * 210)}deg`);
  current.style.setProperty("--pull-scale", (0.86 + pullProgress * 0.14).toFixed(3));
  current.toggleAttribute("aria-hidden", !pullRefreshing);
  current.setAttribute("aria-label", pullRefreshing ? "正在刷新账本" : "");
}

async function triggerPullRefresh() {
  if (autoRefreshInFlight) {
    cancelPullRefresh();
    return;
  }
  pullRefreshing = true;
  pullArmed = false;
  pullProgress = 1;
  pullDistance = pullRefreshThreshold;
  updatePullRefreshIndicator();

  try {
    await refreshRemoteState({ silent: true, allowPending: true, announceSync: true });
    if (state.cloudStatus.state === "online" && !state.reauthRequired) {
      const current = app.querySelector<HTMLElement>("#pullRefreshIndicator");
      current?.classList.add("is-success");
      current?.setAttribute("aria-label", "账本刷新成功");
      await waitForMotion(520);
    }
  } finally {
    pullRefreshing = false;
    pullProgress = 0;
    pullDistance = 0;
    const current = app.querySelector<HTMLElement>("#pullRefreshIndicator");
    current?.classList.remove("is-success");
    updatePullRefreshIndicator();
  }
}

function isInteractivePullTarget(target: EventTarget | null) {
  return (
    target instanceof Element &&
    Boolean(target.closest("button, input, select, textarea, [contenteditable='true'], .user-menu-panel"))
  );
}

function waitForMotion(milliseconds: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));
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
        ${renderCloudStatusChip()}
        ${renderUserMenu()}
      </div>
    </header>
    <div class="ledger-picker" id="ledgerSelect" role="group" aria-label="账本切换">
      <span>账本</span>
      <div class="switcher-row">
        ${state.ledgers.map(renderLedgerSwitchButton).join("")}
      </div>
    </div>
    ${renderSyncBanner()}
  `;
}

interface SyncPresentation {
  tone: "checking" | "online" | "offline" | "syncing" | "success" | "failed";
  icon: string;
  chipLabel: string;
  title: string;
  detail: string;
  visible: boolean;
  progress?: number;
}

function syncPresentation(): SyncPresentation {
  const pending = state.outbox.length;
  if (state.reauthRequired) {
    return {
      tone: "failed",
      icon: "log-in",
      chipLabel: "需要登录",
      title: "登录后继续同步",
      detail: pending > 0 ? `${pending} 笔本地记录已安全保留` : "本地账本仍可查看和记账",
      visible: true,
    };
  }
  if (state.sync.phase === "connecting") {
    return {
      tone: "checking",
      icon: "refresh-cw",
      chipLabel: "正在连接",
      title: "正在连接云端",
      detail: pending > 0 ? `连接后将同步 ${pending} 笔本地记录` : "正在恢复安全会话",
      visible: true,
    };
  }
  if (state.sync.phase === "syncing") {
    const total = Math.max(state.sync.total, 1);
    return {
      tone: "syncing",
      icon: "refresh-cw",
      chipLabel: `同步 ${state.sync.completed}/${state.sync.total}`,
      title: `正在同步 ${state.sync.completed}/${state.sync.total}`,
      detail: "账本可继续使用，记录将按顺序上传",
      visible: true,
      progress: state.sync.completed / total,
    };
  }
  if (state.sync.phase === "success") {
    return {
      tone: "success",
      icon: "circle-check",
      chipLabel: "同步完成",
      title: `${state.sync.syncedCount} 笔记录已同步`,
      detail: "本地记录已安全保存到云端",
      visible: true,
      progress: 1,
    };
  }
  if (state.sync.phase === "failed") {
    return {
      tone: "failed",
      icon: "triangle-alert",
      chipLabel: pending > 0 ? `${pending} 笔待同步` : "连接不稳定",
      title: pending > 0 ? "同步暂未完成" : "云端暂时不可用",
      detail: pending > 0 ? "记录已保存在本机，联网后会自动重试" : "账本仍可离线使用",
      visible: true,
    };
  }
  if (state.cloudStatus.state === "offline") {
    return {
      tone: "offline",
      icon: "cloud-off",
      chipLabel: pending > 0 ? `离线 · ${pending} 笔` : "离线记账",
      title: pending > 0 ? `${pending} 笔记录等待同步` : "当前处于离线状态",
      detail: "记录会先保存在本机，联网后自动同步",
      visible: true,
    };
  }
  return {
    tone: "online",
    icon: "cloud",
    chipLabel: "云端在线",
    title: "云端在线",
    detail: "",
    visible: false,
  };
}

function renderCloudStatusChip() {
  const presentation = syncPresentation();
  return `<span class="cloud-chip ${presentation.tone}" id="cloudStatusChip">
    <i data-lucide="${presentation.icon}" aria-hidden="true"></i>
    <span>${escapeHtml(presentation.chipLabel)}</span>
  </span>`;
}

function renderSyncBanner() {
  const presentation = syncPresentation();
  const progress = Math.max(0, Math.min(1, presentation.progress ?? 0));
  return `<div
    class="sync-banner ${presentation.tone} ${presentation.visible ? "is-visible" : "is-hidden"}"
    id="syncBanner"
    role="status"
    aria-live="polite"
    aria-atomic="true"
    ${presentation.visible ? "" : 'aria-hidden="true"'}
    style="--sync-progress:${progress}"
  >
    <span class="sync-banner-icon" aria-hidden="true"><i data-lucide="${presentation.icon}"></i></span>
    <span class="sync-banner-copy">
      <strong>${escapeHtml(presentation.title)}</strong>
      <span>${escapeHtml(presentation.detail)}</span>
    </span>
    ${
      state.reauthRequired
        ? '<button class="sync-login-button" type="button" data-sync-action="login">重新登录</button>'
        : ""
    }
    ${presentation.progress === undefined ? "" : '<span class="sync-progress" aria-hidden="true"><span></span></span>'}
  </div>`;
}

function updateSyncStatusRegion() {
  const presentationKey = JSON.stringify(syncPresentation());
  if (presentationKey === lastSyncPresentationKey) return;
  const chip = app.querySelector<HTMLElement>("#cloudStatusChip");
  if (chip) chip.outerHTML = renderCloudStatusChip();
  const banner = app.querySelector<HTMLElement>("#syncBanner");
  if (banner) banner.outerHTML = renderSyncBanner();
  createIcons({
    icons,
    attrs: { width: "18", height: "18", "stroke-width": "2" },
  });
  bindSyncStatusEvents();
  lastSyncPresentationKey = presentationKey;
}

function scheduleSyncSettle() {
  if (syncSettleTimer !== undefined) window.clearTimeout(syncSettleTimer);
  syncSettleTimer = window.setTimeout(() => {
    if (state.sync.phase !== "success") return;
    state.sync = { phase: "idle", completed: 0, total: 0, syncedCount: 0 };
    settleRecentlySyncedRows();
    state.recentlySyncedTransactionIds.clear();
    updateSyncStatusRegion();
  }, 1600);
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

function renderLogin(sceneMotionClass: string) {
  return `
    <section class="login-panel view-stage ${sceneMotionClass}" aria-label="账号登录">
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
          ${
            state.pendingAction === "auth"
              ? '<i class="button-spinner" data-lucide="loader-circle" aria-hidden="true"></i>正在登录'
              : "登录"
          }
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

function renderDashboard(dashboard: LedgerDashboard, sceneMotionClass: string) {
  return `
    <section class="dashboard-stage view-stage ${sceneMotionClass}" data-view="${state.view}">
      ${renderBalancePanel(dashboard)}
      ${state.view === "analysis" ? "" : renderQuickEntry(dashboard)}
      ${renderActiveView(dashboard)}
    </section>
  `;
}

function renderBalancePanel(dashboard: LedgerDashboard) {
  const { ledger } = dashboard;
  const canViewBalances = ledger.canViewBalances;
  const pendingPaymentCount = dashboard.recentTransactions.filter(
    (transaction) =>
      transaction.approvalState === "approved" && transaction.paymentState === "pending_payment",
  ).length;
  const pendingReceiptCount = dashboard.recentTransactions.filter(
    (transaction) =>
      transaction.approvalState === "approved" &&
      transaction.paymentState === "paid_pending_receipt",
  ).length;
  const organization = ledger.organizationName
    ? `<span class="context-pill">${escapeHtml(ledger.organizationName)}</span>`
    : "";

  return `
    <section class="balance-panel" aria-label="账本概览">
      <div class="balance-heading">
        <div>
          <span class="section-kicker">${ledgerKindLabel(ledger.kind)}</span>
          <p class="balance-label">${escapeHtml(canViewBalances ? ledger.name : "我的公账申请")}</p>
          ${canViewBalances ? `<strong class="balance-value ${state.amountsVisible ? "is-visible" : "is-hidden"}">${formatMoney(ledger.balanceCents ?? 0, ledger.currency)}</strong>` : ""}
        </div>
        ${canViewBalances ? `<button
          class="amount-visibility-toggle"
          id="amountVisibilityToggle"
          type="button"
          aria-label="${state.amountsVisible ? "隐藏金额" : "显示金额"}"
          aria-pressed="${state.amountsVisible}"
          title="${state.amountsVisible ? "隐藏金额" : "显示金额"}"
        >
          <i data-lucide="${state.amountsVisible ? "eye-off" : "eye"}" aria-hidden="true"></i>
        </button>` : ""}
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
  const tabs = renderAnalysisTabs();

  if (state.analysisTab === "month") {
    return renderMonthlyAnalysisPanel(tabs);
  }

  if (state.analysisLoading && !analysis) {
    return `
      <section class="analysis-view" aria-label="财务分析">
        <div class="analysis-heading">
          <div><span class="section-kicker">Analytics</span><h2>财务分析</h2></div>
          <div class="analysis-heading-actions">${tabs}${rangeControls}</div>
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
          <div class="analysis-heading-actions">${tabs}${rangeControls}</div>
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

  if (state.analysisDetailTarget) {
    return renderAnalysisDetailPanel();
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
        <div class="analysis-heading-actions">${tabs}${rangeControls}</div>
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
        <div class="cashflow-chart" role="group" aria-label="${analysis.months}个月收入和支出，选择月份查看详情">
          ${analysis.trend
            .map(
              (point) => `
                <button class="chart-period" type="button" data-analysis-month="${escapeHtml(point.key)}" title="查看 ${escapeHtml(point.label)} 明细：收入 ${escapeHtml(formatMoney(point.incomeCents, currency))}，支出 ${escapeHtml(formatMoney(point.expenseCents, currency))}">
                  <div class="bar-pair">
                    <span class="chart-bar income" style="--bar-height:${barHeight(point.incomeCents, maxTrendValue)}%"></span>
                    <span class="chart-bar expense" style="--bar-height:${barHeight(point.expenseCents, maxTrendValue)}%"></span>
                  </div>
                  <span>${escapeHtml(point.label)}</span>
                </button>
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
                      <button class="member-spend-row" type="button" data-analysis-member="${escapeHtml(member.userId)}" title="查看 ${escapeHtml(member.displayName)} 已打款明细">
                        <div><strong>${escapeHtml(member.displayName)}</strong><span>${member.transactionCount} 笔</span></div>
                        <div class="member-spend-track"><span style="--spend-width:${Math.max(4, (member.expenseCents / maxMemberExpense) * 100)}%"></span></div>
                        <strong>${formatMoney(member.expenseCents, currency)}</strong>
                      </button>
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

function renderAnalysisTabs() {
  return `
    <div class="analysis-tabs" role="tablist" aria-label="分析类型">
      <button class="filter-tab ${state.analysisTab === "summary" ? "is-active" : ""}" type="button" data-analysis-tab="summary" role="tab" aria-selected="${state.analysisTab === "summary"}">综合分析</button>
      <button class="filter-tab ${state.analysisTab === "month" ? "is-active" : ""}" type="button" data-analysis-tab="month" role="tab" aria-selected="${state.analysisTab === "month"}">月度分析</button>
    </div>
  `;
}

function renderMonthlyAnalysisPanel(tabs: string) {
  const detail = state.analysisMonthDetail?.month === state.analysisMonth ? state.analysisMonthDetail : undefined;
  const heading = `
    <div class="analysis-heading">
      <div>
        <span class="section-kicker">Monthly analytics</span>
        <h2>月度分析</h2>
        <p>按现金实际发生时间统计</p>
      </div>
      <div class="analysis-heading-actions">
        ${tabs}
        <button class="month-picker-button" type="button" data-date-picker-scope="analysis" data-date-picker-granularity="month" aria-label="选择分析月份">
          <i data-lucide="calendar-range" aria-hidden="true"></i>${escapeHtml(formatMonthLabel(state.analysisMonth))}
        </button>
      </div>
    </div>
  `;
  if (state.analysisMonthLoading && !detail) {
    return `<section class="analysis-view" aria-label="月度分析">${heading}<div class="analysis-state"><div class="spinner" aria-hidden="true"></div><p>正在加载月度分析</p></div></section>`;
  }
  if (state.analysisMonthError && !detail) {
    return `<section class="analysis-view" aria-label="月度分析">${heading}<div class="analysis-state"><p>${escapeHtml(state.analysisMonthError)}</p><button class="primary-button" id="retryAnalysisMonthButton" type="button">重新加载</button></div></section>`;
  }
  if (!detail) {
    return `<section class="analysis-view" aria-label="月度分析">${heading}<div class="analysis-state"><p>请选择月份查看分析</p></div></section>`;
  }
  return renderMonthAnalysisDetail(heading, detail);
}

function renderAnalysisDetailPanel() {
  const target = state.analysisDetailTarget;
  if (!target) return "";
  const title =
    target.kind === "month"
      ? `${escapeHtml(target.month)} 月度详情`
      : escapeHtml(
          state.analysisMemberDetail?.displayName ??
            state.analysis?.memberExpenses.find((member) => member.userId === target.memberId)
              ?.displayName ??
            "成员明细",
        );
  const heading = `
    <div class="analysis-heading analysis-detail-heading">
      <div>
        <span class="section-kicker">Detail</span>
        <h2>${title}</h2>
        <p>${target.kind === "month" ? "按现金实际发生时间统计" : `最近 ${state.analysisMonths} 个月已打款项目`}</p>
      </div>
      <button class="ghost-button analysis-back-button" id="closeAnalysisDetailButton" type="button">
        <i data-lucide="arrow-left" aria-hidden="true"></i>返回汇总
      </button>
    </div>
  `;

  if (state.analysisDetailLoading) {
    return `<section class="analysis-view" aria-label="财务分析详情">${heading}<div class="analysis-state"><div class="spinner" aria-hidden="true"></div><p>正在加载明细</p></div></section>`;
  }
  if (state.analysisDetailError) {
    return `
      <section class="analysis-view" aria-label="财务分析详情">
        ${heading}
        <div class="analysis-state">
          <p>${escapeHtml(state.analysisDetailError)}</p>
          <button class="primary-button" id="retryAnalysisDetailButton" type="button">重新加载</button>
        </div>
      </section>
    `;
  }
  if (target.kind === "month" && state.analysisMonthDetail) {
    return renderMonthAnalysisDetail(heading, state.analysisMonthDetail);
  }
  if (target.kind === "member" && state.analysisMemberDetail) {
    return renderMemberAnalysisDetail(heading, state.analysisMemberDetail);
  }
  return `<section class="analysis-view" aria-label="财务分析详情">${heading}<div class="analysis-state"><p>暂无详情数据</p></div></section>`;
}

function renderMonthAnalysisDetail(heading: string, detail: FinancialMonthDetail) {
  const activeMembers = detail.memberExpenses.filter((member) => member.expenseCents > 0);
  return `
    <section class="analysis-view" aria-label="${escapeHtml(detail.month)} 月度详情">
      ${heading}
      <div class="analysis-metrics">
        ${renderAnalysisMetric("月收入", detail.incomeCents, detail.currency, "实际批准入账")}
        ${renderAnalysisMetric("月支出", detail.expenseCents, detail.currency, "实际完成打款")}
        ${renderAnalysisMetric("净现金流", detail.netCashFlowCents, detail.currency, "收入减支出", detail.netCashFlowCents >= 0 ? "positive" : "negative")}
        ${renderAnalysisMetric("实际笔数", detail.transactionCount, "", "当月现金流")}
      </div>
      <section class="analysis-section" aria-label="分类构成">
        <div class="analysis-section-heading"><h3>分类构成</h3><span>${detail.categories.length} 个分类</span></div>
        <div class="analysis-list">
          ${
            detail.categories.length > 0
              ? detail.categories
                  .map(
                    (category) => `<div class="analysis-list-row"><div><strong>${escapeHtml(category.categoryName)}</strong><span>${category.direction === "income" ? "收入" : "支出"} · ${category.transactionCount} 笔</span></div><strong class="${category.direction === "income" ? "amount-in" : "amount-out"}">${formatMoney(category.amountCents, detail.currency)}</strong></div>`,
                  )
                  .join("")
              : `<p class="empty-copy">本月暂无实际收支</p>`
          }
        </div>
      </section>
      <section class="analysis-section" aria-label="成员支出">
        <div class="analysis-section-heading"><h3>成员支出</h3><span>已打款</span></div>
        <div class="analysis-list">
          ${
            activeMembers.length > 0
              ? activeMembers
                  .map(
                    (member) => `<div class="analysis-list-row"><div><strong>${escapeHtml(member.displayName)}</strong><span>${member.transactionCount} 笔</span></div><strong class="amount-out">${formatMoney(member.expenseCents, detail.currency)}</strong></div>`,
                  )
                  .join("")
              : `<p class="empty-copy">本月暂无成员已打款支出</p>`
          }
        </div>
      </section>
      ${renderActualTransactionSection(detail.transactions, detail.currency, "逐笔实际收支")}
    </section>
  `;
}

function renderMemberAnalysisDetail(heading: string, detail: FinancialMemberDetail) {
  return `
    <section class="analysis-view" aria-label="${escapeHtml(detail.displayName)} 已打款项目">
      ${heading}
      <div class="analysis-metrics analysis-member-metrics">
        ${renderAnalysisMetric("已打款支出", detail.expenseCents, detail.currency, `${detail.transactionCount} 笔项目`)}
        ${renderAnalysisMetric("分析周期", detail.months, "", `${formatPeriodMonth(detail.periodStart)} 至 ${formatPeriodMonth(detail.periodEnd)}`)}
      </div>
      ${renderActualTransactionSection(detail.transactions, detail.currency, "已打款项目")}
    </section>
  `;
}

function renderActualTransactionSection(
  transactions: FinancialMonthDetail["transactions"],
  currency: string,
  title: string,
) {
  return `
    <section class="analysis-section" aria-label="${escapeHtml(title)}">
      <div class="analysis-section-heading"><h3>${escapeHtml(title)}</h3><span>${transactions.length} 笔</span></div>
      <div class="analysis-list analysis-transaction-list">
        ${
          transactions.length > 0
            ? transactions
                .map(
                  (transaction) => `
                    <div class="analysis-list-row analysis-transaction-row">
                      <div>
                        <strong>${escapeHtml(transaction.description)}</strong>
                        <span>${escapeHtml(transaction.categoryName)} · ${escapeHtml(transaction.accountName)}</span>
                        <span>${escapeHtml(transaction.submittedBy)} · ${formatDate(transaction.effectiveAt)} · ${analysisPaymentStateLabel(transaction.paymentState)}</span>
                      </div>
                      <strong class="${transaction.direction === "income" ? "amount-in" : "amount-out"}">${transaction.direction === "income" ? "+" : "-"}${formatMoney(transaction.amountCents, currency)}</strong>
                    </div>
                  `,
                )
                .join("")
            : `<p class="empty-copy">所选范围暂无实际收支</p>`
        }
      </div>
    </section>
  `;
}

function analysisPaymentStateLabel(paymentState: FinancialMonthDetail["transactions"][number]["paymentState"]) {
  if (paymentState === "paid_pending_receipt") return "待确认收款";
  if (paymentState === "received") return "已确认收款";
  return "已入账";
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
  const formattedValue = currency ? formatMoney(value, currency) : formatCompactNumber(value);
  return `<article class="analysis-metric ${tone}"><span>${label}</span><strong>${formattedValue}</strong><p>${escapeHtml(detail)}</p></article>`;
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

    if (state.filter === "voided") {
      return transaction.approvalState === "voided";
    }

    return true;
  });
  const filterCounts: Record<TransactionFilter, number> = {
    all: dashboard.recentTransactions.length,
    pending: dashboard.recentTransactions.filter(
      (transaction) => transaction.approvalState === "pending",
    ).length,
    approved: dashboard.recentTransactions.filter(
      (transaction) => transaction.approvalState === "approved",
    ).length,
    rejected: dashboard.recentTransactions.filter(
      (transaction) => transaction.approvalState === "rejected",
    ).length,
    voided: dashboard.recentTransactions.filter(
      (transaction) => transaction.approvalState === "voided",
    ).length,
  };

  return `
    <section class="activity-panel" aria-label="流水列表">
      <div class="section-heading activity-heading">
        <div>
          <span class="section-kicker">Activity</span>
          <h2>流水</h2>
        </div>
        <span class="activity-total"><strong>${filtered.length}</strong><span>笔记录</span></span>
      </div>
      <div class="activity-toolbar">
        <div class="activity-range-control">
          <span class="toolbar-label">查看范围</span>
          <div class="period-toggle" role="group" aria-label="流水时间范围">
            ${renderPeriodButton("activity", "month", "月")}
            ${renderPeriodButton("activity", "day", "日")}
          </div>
        </div>
        ${renderDatePickerTrigger(
          "activity",
          state.activityGranularity,
          state.activityGranularity === "day" ? state.activityDay : state.activityMonth,
        )}
        <span class="activity-range-note">${state.activityGranularity === "day" ? "当天流水" : "整月流水"}</span>
      </div>
      <div class="activity-filter-row">
        <span class="toolbar-label">状态</span>
        <div class="filter-tabs" role="tablist" aria-label="流水筛选">
          ${renderFilterButton("all", "全部", filterCounts.all)}
          ${renderFilterButton("pending", "待审", filterCounts.pending)}
          ${renderFilterButton("approved", "已入账", filterCounts.approved)}
          ${renderFilterButton("rejected", "驳回", filterCounts.rejected)}
          ${renderFilterButton("voided", "作废", filterCounts.voided)}
        </div>
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

function renderDatePickerTrigger(
  scope: DatePickerScope,
  granularity: PeriodGranularity,
  value: string,
) {
  const isDay = granularity === "day";
  return `
    <div class="activity-period-input">
      <span>${isDay ? "选择日期" : "选择月份"}</span>
      <button
        class="date-picker-trigger"
        type="button"
        data-date-picker-scope="${scope}"
        data-date-picker-granularity="${granularity}"
        aria-haspopup="dialog"
        aria-expanded="${state.datePicker?.scope === scope && state.datePicker.granularity === granularity}"
      >
        <span class="date-picker-trigger-icon" aria-hidden="true"><i data-lucide="${isDay ? "calendar-days" : "calendar-range"}"></i></span>
        <span class="date-picker-trigger-copy"><strong>${escapeHtml(isDay ? formatDayLabel(value) : formatMonthLabel(value))}</strong><small>${isDay ? "按天查看" : "按月查看"}</small></span>
        <i class="date-picker-trigger-chevron" data-lucide="chevron-down" aria-hidden="true"></i>
      </button>
    </div>
  `;
}

function renderFilterButton(filter: TransactionFilter, label: string, count?: number) {
  const active = state.filter === filter;

  return `
    <button
      class="filter-tab ${active ? "is-active" : ""}"
      type="button"
      data-filter="${filter}"
      aria-selected="${active}"
    >
      <span>${label}</span>${count === undefined ? "" : `<small>${count}</small>`}
    </button>
  `;
}

function renderPeriodButton(
  scope: "activity" | "audit",
  granularity: PeriodGranularity,
  label: string,
) {
  const active =
    scope === "activity"
      ? state.activityGranularity === granularity
      : state.auditGranularity === granularity;
  return `<button class="period-button ${active ? "is-active" : ""}" type="button" data-${scope}-granularity="${granularity}" aria-pressed="${active}">${label}</button>`;
}

function renderTransactionRow(
  transaction: LedgerDashboard["recentTransactions"][number],
  dashboard: LedgerDashboard,
) {
  const currency = dashboard.ledger.currency;
  const signedAmount = formatSignedMoney(transaction.amountCents, currency, transaction.direction);
  const unsynced = isUnsyncedTransaction(transaction.id);
  const recentlySynced = state.recentlySyncedTransactionIds.has(transaction.id);
  const directionIcon = transaction.direction === "expense" ? "arrow-down-left" : "arrow-up-right";

  return `
    <article class="transaction-row ${recentlySynced ? "is-just-synced" : ""}" data-transaction-id="${escapeHtml(transaction.id)}">
      <div class="row-main transaction-row-head">
        <div class="transaction-leading">
          <span class="transaction-icon ${transaction.direction === "expense" ? "is-expense" : "is-income"}" aria-hidden="true"><i data-lucide="${directionIcon}"></i></span>
          <div class="transaction-copy">
            <h3>${escapeHtml(transaction.title)}</h3>
            <p>${escapeHtml(transaction.accountName)} · ${escapeHtml(transaction.categoryName)}</p>
          </div>
        </div>
        <strong class="${transaction.direction === "expense" ? "amount-out" : "amount-in"}">
          ${signedAmount}
        </strong>
      </div>
      <div class="row-meta transaction-row-foot">
        <span class="transaction-date"><i data-lucide="calendar-days" aria-hidden="true"></i>${formatDate(transaction.occurredAt)}</span>
        ${
          unsynced
            ? '<span class="status-chip is-unsynced">未同步</span>'
            : recentlySynced
              ? '<span class="status-chip is-synced"><i data-lucide="check" aria-hidden="true"></i>已同步</span>'
            : `<span class="status-chip ${statusClass(transaction.approvalState)}">
                ${transactionStateLabel(transaction)}
              </span>`
        }
      </div>
      ${renderTransactionActions(transaction, dashboard)}
    </article>
  `;
}

function renderTransactionActions(
  transaction: LedgerDashboard["recentTransactions"][number],
  dashboard: LedgerDashboard,
) {
  if (isUnsyncedTransaction(transaction.id)) {
    return "";
  }
  const canMarkPaid =
    dashboard.ledger.role === "business_owner" &&
    transaction.approvalState === "approved" &&
    transaction.paymentState === "pending_payment";
  const canConfirmReceipt =
    transaction.approvalState === "approved" &&
    transaction.paymentState === "paid_pending_receipt" &&
    transaction.createdByUserId === state.currentUser?.id;
  const canVoid =
    dashboard.ledger.kind === "organization" &&
    dashboard.ledger.role === "business_owner" &&
    state.cloudStatus.state === "online" &&
    transaction.approvalState === "approved";
  if (!canMarkPaid && !canConfirmReceipt && !canVoid) {
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
    ${
      canVoid
        ? `<button class="danger-button" type="button" data-void-transaction="${escapeHtml(transaction.id)}">作废</button>`
        : ""
    }
  </div>`;
}

function isUnsyncedTransaction(transactionId: string) {
  return state.outbox.some((item) => item.localId === transactionId);
}

function updateTransactionSyncBadge(localId: string, phase: "syncing" | "synced") {
  for (const row of app.querySelectorAll<HTMLElement>("[data-transaction-id]")) {
    if (row.dataset.transactionId !== localId) continue;
    const badge = row.querySelector<HTMLElement>(".status-chip");
    if (!badge) return;
    badge.className = `status-chip is-${phase}`;
    badge.textContent = phase === "syncing" ? "同步中" : "已同步";
    if (phase === "synced") row.classList.add("is-just-synced");
    return;
  }
}

function settleRecentlySyncedRows() {
  const dashboard = state.dashboard;
  if (!dashboard) return;
  for (const row of app.querySelectorAll<HTMLElement>("[data-transaction-id]")) {
    const transaction = dashboard.recentTransactions.find(
      (item) => item.id === row.dataset.transactionId,
    );
    if (!transaction || !state.recentlySyncedTransactionIds.has(transaction.id)) continue;
    const badge = row.querySelector<HTMLElement>(".status-chip");
    if (!badge) continue;
    badge.className = `status-chip ${statusClass(transaction.approvalState)}`;
    badge.textContent = transactionStateLabel(transaction);
    row.classList.remove("is-just-synced");
  }
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
  const signedAmount = formatSignedMoney(item.amountCents, currency, item.direction);
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

function renderAuditPanel(_dashboard: LedgerDashboard) {
  const detail = state.auditData?.lifecycles.find(
    (item) => item.transactionId === state.auditDetailTransactionId,
  );
  if (detail) return renderAuditLifecycleDetail(detail);

  const audit = state.auditData;
  return `
    <section class="activity-panel" aria-label="审计入口">
      <div class="section-heading">
        <div>
          <span class="section-kicker">Audit</span>
          <h2>审计</h2>
        </div>
        <span class="count-badge">${audit?.lifecycles.length ?? 0}</span>
      </div>
      <div class="activity-toolbar audit-period-bar">
        <div class="activity-range-control">
          <span class="toolbar-label">查看范围</span>
          <div class="period-toggle" role="group" aria-label="审计时间范围">
            ${renderPeriodButton("audit", "month", "月")}
            ${renderPeriodButton("audit", "day", "日")}
          </div>
        </div>
        ${renderDatePickerTrigger("audit", state.auditGranularity, state.auditPeriod)}
        <span class="activity-range-note">${audit?.lifecycles.length ?? 0} 笔流水</span>
      </div>
      ${state.auditLoading ? '<div class="audit-state"><div class="spinner" aria-hidden="true"></div><p>正在加载审计生命周期</p></div>' : ""}
      ${state.auditError ? `<div class="audit-state"><p>${escapeHtml(state.auditError)}</p><button class="primary-button" id="retryAuditButton" type="button">重新加载</button></div>` : ""}
      <div class="audit-list">
        ${
          audit && audit.lifecycles.length > 0
            ? renderAuditLifecycleGroups(audit.lifecycles)
            : `<p class="empty-copy">暂无审计记录</p>`
        }
      </div>
    </section>
  `;
}

function renderAuditLifecycleGroups(lifecycles: TransactionAuditLifecycle[]) {
  const groups = new Map<string, TransactionAuditLifecycle[]>();
  for (const lifecycle of lifecycles) {
    const key = transactionDayKey(lifecycle.latestAt);
    groups.set(key, [...(groups.get(key) ?? []), lifecycle]);
  }
  return Array.from(groups.entries())
    .sort(([left], [right]) => right.localeCompare(left))
    .map(
      ([day, items]) => `
        <section class="audit-day-group" aria-label="${escapeHtml(formatDayLabel(day))}">
          <h3 class="audit-day-heading">${escapeHtml(formatDayLabel(day))}</h3>
          ${items.map(renderAuditLifecycleCard).join("")}
        </section>
      `,
    )
    .join("");
}

function renderAuditLifecycleCard(lifecycle: TransactionAuditLifecycle) {
  const signedAmount = formatSignedMoney(
    lifecycle.amountCents,
    lifecycle.currency,
    lifecycle.direction,
  );
  const directionIcon = lifecycle.direction === "expense" ? "arrow-down-left" : "arrow-up-right";
  return `
    <button class="audit-row audit-lifecycle-row" type="button" data-audit-transaction="${escapeHtml(lifecycle.transactionId)}">
      <span class="audit-card-marker ${lifecycle.direction === "expense" ? "is-expense" : "is-income"}" aria-hidden="true"><i data-lucide="${directionIcon}"></i></span>
      <span class="audit-row-copy">
        <span class="audit-row-main"><span class="audit-title-line"><strong>${escapeHtml(lifecycle.description)}</strong><i data-lucide="chevron-right" aria-hidden="true"></i></span><strong class="${lifecycle.direction === "expense" ? "amount-out" : "amount-in"}">${signedAmount}</strong></span>
        <span class="audit-row-meta"><span><i data-lucide="clock-3" aria-hidden="true"></i>${formatDate(lifecycle.latestAt)}</span><span class="audit-step-count">${lifecycle.steps.length} 个步骤</span><span class="status-chip ${statusClass(lifecycle.approvalState)}">${approvalStateLabel(lifecycle.approvalState)}</span></span>
        <span class="audit-row-hint">发生于 ${formatDate(lifecycle.occurredAt)} · 查看完整生命周期</span>
      </span>
    </button>
  `;
}

function renderAuditLifecycleDetail(lifecycle: TransactionAuditLifecycle) {
  return `
    <section class="activity-panel audit-detail-panel" aria-label="流水生命周期">
      <div class="section-heading audit-detail-heading">
        <div>
          <span class="section-kicker">Lifecycle</span>
          <h2>${escapeHtml(lifecycle.description)}</h2>
          <p>${formatSignedMoney(lifecycle.amountCents, lifecycle.currency, lifecycle.direction)} · 发生于 ${formatDate(lifecycle.occurredAt)}</p>
        </div>
        <button class="ghost-button" id="closeAuditDetailButton" type="button" title="返回审计列表" aria-label="返回审计列表"><i data-lucide="arrow-left" aria-hidden="true"></i>返回</button>
      </div>
      <div class="audit-lifecycle-summary">
        <span><i data-lucide="clipboard-check" aria-hidden="true"></i>审批：${approvalStateLabel(lifecycle.approvalState)}</span>
        <span><i data-lucide="wallet-cards" aria-hidden="true"></i>付款：${paymentStateLabel(lifecycle.paymentState)}</span>
        <span><i data-lucide="route" aria-hidden="true"></i>${lifecycle.steps.length} 个审计步骤</span>
      </div>
      <div class="audit-timeline">
        ${lifecycle.steps
          .map(
            (step, index) => `
              <article class="audit-timeline-step">
                <span class="audit-step-marker" aria-hidden="true"><i data-lucide="${auditActionIcon(step.action)}"></i></span>
                <div>
                  <h3>${escapeHtml(auditActionLabel(step.action))}</h3>
                  <p>${escapeHtml(step.actorName)} · ${formatDate(step.createdAt)}</p>
                  <p>${escapeHtml(step.summary)}</p>
                </div>
                <span class="audit-step-index">${String(index + 1).padStart(2, "0")}</span>
              </article>
            `,
          )
          .join("")}
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
      ${renderNavButton("audit", "审计", auditLifecycleCount(dashboard))}
    </nav>
  `;
}

function auditLifecycleCount(dashboard: LedgerDashboard) {
  return new Set(dashboard.auditTrail.map((item) => item.resourceId)).size;
}

function renderNavButton(view: ViewMode, label: string, count?: number) {
  const active = state.view === view;
  const iconsByView: Record<ViewMode, string> = {
    activity: "receipt",
    analysis: "chart-column",
    approval: "clipboard-check",
    audit: "shield-check",
  };

  return `
    <button class="nav-button ${active ? "is-active" : ""}" type="button" data-view-target="${view}">
      <span class="nav-icon" aria-hidden="true"><i data-lucide="${iconsByView[view]}"></i></span>
      <span>${label}</span>
      <span class="nav-count ${count === undefined ? "is-empty" : ""}">${count ?? ""}</span>
    </button>
  `;
}

function renderLoading(sceneMotionClass: string) {
  return `
    <section class="state-panel view-stage ${sceneMotionClass}">
      <div class="spinner" aria-hidden="true"></div>
      <p>正在加载账本</p>
    </section>
  `;
}

function renderError(sceneMotionClass: string) {
  return `
    <section class="state-panel view-stage ${sceneMotionClass}">
      <p>${escapeHtml(state.error ?? "加载失败")}</p>
      <button class="primary-button" type="button" id="retryButton">重试</button>
    </section>
  `;
}

function renderEmptyState(sceneMotionClass: string) {
  return `
    <section class="state-panel view-stage ${sceneMotionClass}">
      <p>暂无账本</p>
    </section>
  `;
}

function bindEvents() {
  bindSyncStatusEvents();

  app.querySelector<HTMLButtonElement>("#openUpdateDownloadButton")?.addEventListener("click", () => {
    const url = state.updateRequired?.downloadUrl;
    if (url) openDownloadUrl(url);
  });

  const picker = state.datePicker;
  const pickerContent = app.querySelector<HTMLElement>(".date-picker-content");
  if (picker?.transition && pickerContent) {
    const { transition, viewMonth } = picker;
    pickerContent.addEventListener(
      "animationend",
      (event) => {
        if (
          event.target === pickerContent &&
          state.datePicker?.viewMonth === viewMonth &&
          state.datePicker.transition === transition
        ) {
          state.datePicker = { ...state.datePicker, transition: undefined };
        }
      },
      { once: true },
    );
  }

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

  app.querySelector<HTMLButtonElement>("#amountVisibilityToggle")?.addEventListener("click", () => {
    state.amountsVisible = !state.amountsVisible;
    render();
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

  app.querySelectorAll<HTMLButtonElement>("[data-date-picker-scope]").forEach((button) => {
    button.addEventListener("click", () => {
      const scope = button.dataset.datePickerScope;
      const granularity = button.dataset.datePickerGranularity;
      if (
        (scope === "activity" || scope === "audit" || scope === "analysis") &&
        (granularity === "day" || granularity === "month") &&
        !(scope === "analysis" && granularity !== "month")
      ) {
        openDatePicker(scope, granularity);
      }
    });
  });

  app.querySelector<HTMLElement>("[data-picker-backdrop]")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeDatePicker();
  });
  app.querySelectorAll<HTMLButtonElement>("[data-picker-close]").forEach((button) => {
    button.addEventListener("click", closeDatePicker);
  });
  app.querySelector<HTMLButtonElement>("[data-picker-confirm]")?.addEventListener("click", confirmDatePicker);
  app.querySelector<HTMLButtonElement>("[data-picker-today]")?.addEventListener("click", setDatePickerToday);
  app.querySelectorAll<HTMLButtonElement>("[data-picker-nav]").forEach((button) => {
    button.addEventListener("click", () => {
      const direction = button.dataset.pickerNav;
      if (direction === "previous" || direction === "next") navigateDatePicker(direction);
    });
  });
  app.querySelectorAll<HTMLButtonElement>("[data-picker-value]").forEach((button) => {
    button.addEventListener("click", () => {
      const value = button.dataset.pickerValue;
      if (value) updateDatePickerDraft(value);
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-activity-granularity]").forEach((button) => {
    button.addEventListener("click", () => {
      const granularity = button.dataset.activityGranularity;
      if (granularity === "day" || granularity === "month") {
        void changeActivityGranularity(granularity);
      }
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-audit-granularity]").forEach((button) => {
    button.addEventListener("click", () => {
      const granularity = button.dataset.auditGranularity;
      if (granularity === "day" || granularity === "month") {
        const period = granularity === "day" ? currentDayKey() : currentMonthKey();
        setAuditPeriod(granularity, period);
      }
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-audit-transaction]").forEach((button) => {
    button.addEventListener("click", () => {
      const transactionId = button.dataset.auditTransaction;
      if (transactionId) {
        state.auditDetailTransactionId = transactionId;
        render();
      }
    });
  });

  app.querySelector<HTMLButtonElement>("#closeAuditDetailButton")?.addEventListener("click", () => {
    state.auditDetailTransactionId = undefined;
    render();
  });

  app.querySelector<HTMLButtonElement>("#retryAuditButton")?.addEventListener("click", () => {
    void loadAuditPeriod(true);
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
        resetAnalysisDetail();
        void loadFinancialAnalysis(true);
      }
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-analysis-tab]").forEach((button) => {
    button.addEventListener("click", () => {
      const tab = button.dataset.analysisTab;
      if (tab !== "summary" && tab !== "month") return;
      state.analysisTab = tab;
      if (tab === "summary") {
        resetAnalysisDetail();
      } else {
        resetAnalysisDetail();
        void loadAnalysisMonth(state.analysisMonth);
      }
      render();
    });
  });

  app.querySelector<HTMLButtonElement>("#retryAnalysisButton")?.addEventListener("click", () => {
    void loadFinancialAnalysis(true);
  });

  app.querySelectorAll<HTMLButtonElement>("[data-analysis-month]").forEach((button) => {
    button.addEventListener("click", () => {
      const month = button.dataset.analysisMonth;
      if (month) {
        state.analysisTab = "month";
        resetAnalysisDetail();
        void loadAnalysisMonth(month);
      }
    });
  });

  app.querySelector<HTMLButtonElement>("#retryAnalysisMonthButton")?.addEventListener("click", () => {
    void loadAnalysisMonth(state.analysisMonth, true);
  });

  app.querySelectorAll<HTMLButtonElement>("[data-analysis-member]").forEach((button) => {
    button.addEventListener("click", () => {
      const memberId = button.dataset.analysisMember;
      if (memberId) void loadAnalysisDetail({ kind: "member", memberId });
    });
  });

  app.querySelector<HTMLButtonElement>("#closeAnalysisDetailButton")?.addEventListener("click", () => {
    closeAnalysisDetail();
  });

  app.querySelector<HTMLButtonElement>("#retryAnalysisDetailButton")?.addEventListener("click", () => {
    if (state.analysisDetailTarget) void loadAnalysisDetail(state.analysisDetailTarget, true);
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

  app.querySelectorAll<HTMLButtonElement>("[data-void-transaction]").forEach((button) => {
    button.addEventListener("click", () => {
      const transactionId = button.dataset.voidTransaction;
      if (transactionId) openVoidDialog(transactionId);
    });
  });
  app.querySelector<HTMLElement>("[data-void-backdrop]")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeVoidDialog();
  });
  app.querySelectorAll<HTMLButtonElement>("[data-void-cancel]").forEach((button) => {
    button.addEventListener("click", closeVoidDialog);
  });
  app.querySelector<HTMLTextAreaElement>("#voidReasonInput")?.addEventListener("input", (event) => {
    if (!state.voidDialog) return;
    state.voidDialog.reason = (event.currentTarget as HTMLTextAreaElement).value;
    const confirmButton = app.querySelector<HTMLButtonElement>("[data-void-confirm]");
    if (confirmButton) confirmButton.disabled = !state.voidDialog.reason.trim();
  });
  app.querySelector<HTMLButtonElement>("[data-void-confirm]")?.addEventListener("click", () => {
    void submitVoidTransaction();
  });

  app.querySelector<HTMLButtonElement>("#retryButton")?.addEventListener("click", () => {
    void loadInitialState();
  });
}

function openDownloadUrl(url: string) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:") return;
    const opened = window.open(parsed.toString(), "_blank", "noopener,noreferrer");
    if (!opened) window.location.assign(parsed.toString());
  } catch {
    showToast("官方下载地址无效");
  }
}

function bindSyncStatusEvents() {
  app.querySelector<HTMLButtonElement>("[data-sync-action='login']")?.addEventListener(
    "click",
    () => {
      state.authStatus = "anonymous";
      state.loading = false;
      state.userMenuOpen = false;
      state.profileEditing = false;
      render();
    },
  );
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

function openVoidDialog(transactionId: string) {
  const dashboard = state.dashboard;
  const transaction = dashboard?.recentTransactions.find((item) => item.id === transactionId);
  if (!dashboard || !transaction || state.pendingAction) return;
  if (
    dashboard.ledger.kind !== "organization" ||
    dashboard.ledger.role !== "business_owner" ||
    transaction.approvalState !== "approved"
  ) {
    return;
  }
  if (state.cloudStatus.state !== "online") {
    showToast("联网后才能作废公账流水");
    return;
  }
  state.voidDialog = { transactionId, reason: "" };
  render();
  app.querySelector<HTMLTextAreaElement>("#voidReasonInput")?.focus();
}

function closeVoidDialog() {
  if (state.pendingAction && state.voidDialog?.transactionId === state.pendingAction) return;
  state.voidDialog = undefined;
  render();
}

async function submitVoidTransaction() {
  const dialog = state.voidDialog;
  if (!dialog || state.pendingAction) return;
  const reason = dialog.reason.trim();
  if (!reason) {
    showToast("请输入作废原因");
    return;
  }
  try {
    state.loading = true;
    state.pendingAction = dialog.transactionId;
    render();
    const transaction = await cloudLedgerApi.voidTransaction(dialog.transactionId, reason);
    state.voidDialog = undefined;
    state.analysis = undefined;
    state.analysisError = undefined;
    resetAnalysisDetail();
    state.auditData = undefined;
    state.auditDetailTransactionId = undefined;
    state.cachedDashboards = {};
    state.cachedAuditPeriods = {};
    state.activityMonth = transactionMonthKey(transaction.occurredAt);
    state.activityDay = transactionDayKey(transaction.occurredAt);
    await refreshDashboard();
    state.view = "activity";
    state.filter = "voided";
    showToast("已作废流水，已从余额和分析中排除");
  } catch (error) {
    showToast(friendlyError(error, "作废流水失败"));
  } finally {
    state.loading = false;
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
      state.activityGranularity === "day" ? state.activityDay : undefined,
    );
    rememberDashboard(state.dashboard);
    state.form.categoryId = category.id;
    state.categoryEditing = false;
    state.categoryName = "";
    await saveOfflineSnapshot();
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
  const clientMutationId = newClientMutationId();

  try {
    state.pendingAction = "create";
    render();
    if (state.cloudStatus.state === "offline") {
      await queueOfflineTransaction(draft, clientMutationId);
      clearQuickEntryForm();
      state.view = "activity";
      showToast("已保存到本机，联网后自动同步");
      return;
    }

    const transaction = await cloudLedgerApi.createTransaction(draft, clientMutationId);
    state.analysis = undefined;
    clearQuickEntryForm();
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
    if (isNetworkFailure(error)) {
      try {
        await queueOfflineTransaction(draft, clientMutationId);
        clearQuickEntryForm();
        state.view = "activity";
        showToast("网络不可用，已保存到本机并等待同步");
      } catch (storeError) {
        showToast(friendlyError(storeError, "本地保存失败"));
      }
    } else {
      showToast(friendlyError(error, "保存失败"));
    }
  } finally {
    state.pendingAction = undefined;
    render();
  }
}

async function queueOfflineTransaction(draft: NewTransactionDraft, clientMutationId: string) {
  const visibleDashboard = state.dashboard;
  const user = state.currentUser;
  if (!visibleDashboard || !user) {
    throw new Error("本地账本尚未就绪");
  }
  const targetMonth = transactionMonthKey(draft.occurredAt);
  const dashboard =
    state.cachedDashboards[dashboardCacheKey(draft.ledgerId, targetMonth)] ??
    (visibleDashboard.selectedTransactionMonth === targetMonth
      ? visibleDashboard
      : {
          ...visibleDashboard,
          selectedTransactionMonth: targetMonth,
          recentTransactions: [],
        });
  const account = dashboard.accounts.find((item) => item.id === draft.accountId);
  const category = dashboard.categories.find((item) => item.id === draft.categoryId);
  if (!account || !category) {
    throw new Error("本地账户或分类已失效，请联网后刷新账本");
  }

  const queued: OfflineTransaction = {
    localId: `local-${clientMutationId}`,
    clientMutationId,
    draft,
    createdAt: new Date().toISOString(),
  };
  const localTransaction = {
    id: queued.localId,
    ledgerId: draft.ledgerId,
    occurredAt: draft.occurredAt,
    title: draft.memo || category.name,
    amountCents: draft.amountCents,
    direction: draft.direction,
    accountName: account.name,
    categoryName: category.name,
    approvalState: dashboard.ledger.kind === "organization" ? "pending" as const : "approved" as const,
    paymentState: "not_applicable" as const,
    actorName: user.displayName,
    createdByUserId: user.id,
    memo: draft.memo,
    auditRequired: dashboard.ledger.kind === "organization",
  };
  const targetDay = transactionDayKey(draft.occurredAt);
  const shouldShowLocally =
    dashboard.selectedTransactionMonth === targetMonth &&
    (!dashboard.selectedTransactionDay || dashboard.selectedTransactionDay === targetDay);
  const updatedDashboard: LedgerDashboard = {
    ...dashboard,
    recentTransactions: shouldShowLocally
      ? [localTransaction, ...dashboard.recentTransactions]
      : dashboard.recentTransactions,
    availableTransactionMonths: dashboard.availableTransactionMonths.includes(
      transactionMonthKey(draft.occurredAt),
    )
      ? dashboard.availableTransactionMonths
      : [transactionMonthKey(draft.occurredAt), ...dashboard.availableTransactionMonths],
    availableTransactionDays:
      dashboard.selectedTransactionMonth === targetMonth &&
      !dashboard.availableTransactionDays.includes(targetDay)
        ? [targetDay, ...dashboard.availableTransactionDays]
        : dashboard.availableTransactionDays,
  };
  state.dashboard = updatedDashboard;
  state.activityMonth = targetMonth;
  rememberDashboard(updatedDashboard);
  state.outbox = [...state.outbox, queued];
  state.analysis = undefined;
  state.cloudStatus = {
    state: "offline",
    label: "离线可记账",
    detail: "正在使用本地账本，恢复网络后会自动同步。",
  };
  state.sync = { phase: "idle", completed: 0, total: state.outbox.length, syncedCount: 0 };
  updateCloudStatusLabel();
  await saveOfflineSnapshot();
}

function clearQuickEntryForm() {
  state.form.amount = "";
  state.form.memo = "";
}

function isNetworkFailure(error: unknown) {
  return error instanceof Error && error.message.includes("无法连接后端");
}

function newClientMutationId() {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  return `00000000-0000-4000-8000-${Date.now().toString(16).padStart(12, "0").slice(-12)}`;
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
  if (applyClientUpdate(error)) {
    return "客户端版本过低，请升级后继续使用";
  }
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
  if (message.includes("void reason is required")) {
    return "请输入作废原因";
  }
  if (message.includes("void reason must not exceed 200 characters")) {
    return "作废原因不能超过 200 个字符";
  }
  if (message.includes("only organization public-ledger transactions can be voided")) {
    return "只能作废公账流水";
  }
  if (message.includes("transaction is not eligible for voiding")) {
    return "这笔流水当前不能作废，只能作废已批准流水";
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
  if (!state.amountsVisible) {
    return "****";
  }
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

function formatSignedMoney(
  cents: number,
  currency: string,
  direction: TransactionDirection,
) {
  if (!state.amountsVisible) {
    return "****";
  }
  return `${direction === "expense" ? "-" : "+"}${formatMoney(cents, currency)}`;
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
  return transactionDayKey(new Date().toISOString()).slice(0, 7);
}

function shiftMonthKey(value: string, delta: number) {
  const [year, month] = value.split("-").map(Number);
  const shifted = new Date(Date.UTC(year, month - 1 + delta, 1));
  return `${shifted.getUTCFullYear()}-${String(shifted.getUTCMonth() + 1).padStart(2, "0")}`;
}

function daysInMonthKey(value: string) {
  const [year, month] = value.split("-").map(Number);
  return new Date(Date.UTC(year, month, 0)).getUTCDate();
}

function transactionMonthKey(value: string) {
  return transactionDayKey(value).slice(0, 7);
}

function transactionDayKey(value: string) {
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone: "Asia/Shanghai",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(new Date(value));
  const get = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((part) => part.type === type)?.value ?? "";
  return `${get("year")}-${get("month")}-${get("day")}`;
}

function currentDayKey() {
  return transactionDayKey(new Date().toISOString());
}

function formatDayLabel(day: string) {
  const [year, month, date] = day.split("-");
  return `${year}年${Number(month)}月${Number(date)}日`;
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
  if (transaction.approvalState === "voided") return "已作废";
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
    voided: "is-voided",
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
    transaction_voided: "作废",
  };

  return labels[action];
}

function auditActionIcon(action: LedgerDashboard["auditTrail"][number]["action"]) {
  const icons: Record<LedgerDashboard["auditTrail"][number]["action"], string> = {
    transaction_created: "plus-circle",
    transaction_submitted: "send",
    transaction_approved: "circle-check",
    transaction_rejected: "circle-x",
    transaction_paid: "banknote",
    transaction_received: "hand-coins",
    transaction_auto_approved: "badge-check",
    transaction_voided: "ban",
  };
  return icons[action];
}

function approvalStateLabel(stateValue: ApprovalState) {
  const labels: Record<ApprovalState, string> = {
    draft: "草稿",
    pending: "待审批",
    approved: "已批准",
    rejected: "已驳回",
    voided: "已作废",
  };
  return labels[stateValue];
}

function paymentStateLabel(stateValue: LedgerDashboard["recentTransactions"][number]["paymentState"]) {
  const labels = {
    not_applicable: "无需付款",
    pending_payment: "待打款",
    paid_pending_receipt: "待确认收款",
    received: "已完成",
  } as const;
  return labels[stateValue];
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
