export type LedgerKind = "private" | "organization";

export type MemberRole =
  | "owner"
  | "admin"
  | "business_owner"
  | "employee";

export type TransactionDirection = "expense" | "income";

export type ApprovalState = "draft" | "pending" | "approved" | "rejected" | "deleted";
export type PaymentState = "not_applicable" | "pending_payment" | "paid_pending_receipt" | "received";
export type AnalysisMonths = 3 | 6 | 12;

export type CloudConnectionState = "checking" | "online" | "offline";

export type AuditAction =
  | "transaction_created"
  | "transaction_submitted"
  | "transaction_approved"
  | "transaction_rejected"
  | "transaction_paid"
  | "transaction_received"
  | "transaction_auto_approved"
  | "transaction_deleted";

export interface Ledger {
  id: string;
  name: string;
  kind: LedgerKind;
  currency: "CNY" | "USD" | "EUR" | string;
  role: MemberRole;
  organizationName?: string;
  balanceCents: number;
  pendingCount: number;
  auditUnreadCount: number;
  lastSyncedAt?: string;
}

export interface UserAccount {
  id: string;
  displayName: string;
  email?: string;
  phone?: string;
}

export interface CloudStatus {
  state: CloudConnectionState;
  label: string;
  detail?: string;
}

export interface UserSession {
  currentUser: UserAccount;
  cloudStatus: CloudStatus;
}

export interface AuthSession {
  user: UserAccount;
  accessToken: string;
  refreshToken?: string;
  installationId: string;
}

export interface LoginDraft {
  identifier: string;
  password: string;
  turnstileToken?: string;
}

export interface UpdateProfileDraft {
  displayName: string;
}

export interface FinancialAccount {
  id: string;
  ledgerId: string;
  name: string;
  kind:
    | "wechat"
    | "alipay"
    | "cash"
    | "bank"
    | "wallet"
    | "company"
    | "receivable"
    | "payable";
  balanceCents: number;
}

export interface Category {
  id: string;
  ledgerId: string;
  name: string;
  direction: TransactionDirection;
}

export interface Transaction {
  id: string;
  ledgerId: string;
  occurredAt: string;
  title: string;
  amountCents: number;
  direction: TransactionDirection;
  accountName: string;
  categoryName: string;
  approvalState: ApprovalState;
  paymentState: PaymentState;
  actorName: string;
  createdByUserId: string;
  paidAt?: string;
  receivedAt?: string;
  memo?: string;
  auditRequired: boolean;
}

export interface NewTransactionDraft {
  ledgerId: string;
  occurredAt: string;
  direction: TransactionDirection;
  amountCents: number;
  accountId: string;
  categoryId: string;
  memo?: string;
  submitForApproval: boolean;
}

export interface OfflineTransaction {
  localId: string;
  clientMutationId: string;
  draft: NewTransactionDraft;
  createdAt: string;
}

export interface ApprovalQueueItem {
  id: string;
  transactionId: string;
  ledgerId: string;
  title: string;
  amountCents: number;
  direction: TransactionDirection;
  submittedBy: string;
  submittedById: string;
  submittedAt: string;
  state: Extract<ApprovalState, "pending" | "rejected">;
  canDecide: boolean;
}

export interface AuditLogEntry {
  id: string;
  ledgerId: string;
  action: AuditAction;
  actorName: string;
  resourceId: string;
  createdAt: string;
  summary: string;
}

export interface LedgerDashboard {
  ledger: Ledger;
  accounts: FinancialAccount[];
  categories: Category[];
  selectedTransactionMonth: string;
  availableTransactionMonths: string[];
  recentTransactions: Transaction[];
  approvalQueue: ApprovalQueueItem[];
  auditTrail: AuditLogEntry[];
}

export interface FinancialAnalysis {
  ledgerId: string;
  currency: string;
  months: AnalysisMonths;
  periodStart: string;
  periodEnd: string;
  currentBalanceCents: number;
  incomeCents: number;
  expenseCents: number;
  netCashFlowCents: number;
  previousIncomeCents: number;
  previousExpenseCents: number;
  previousNetCashFlowCents: number;
  transactionCount: number;
  pendingApproval: FinancialExposure;
  pendingPayment: FinancialExposure;
  paidPendingReceipt: FinancialExposure;
  trend: CashFlowTrendPoint[];
  accounts: AnalysisAccount[];
  memberExpenses: MemberExpense[];
  largestExpenses: AnalysisExpense[];
  generatedAt: string;
}

export interface FinancialExposure {
  count: number;
  amountCents: number;
}

export interface CashFlowTrendPoint {
  key: string;
  label: string;
  incomeCents: number;
  expenseCents: number;
  netCashFlowCents: number;
}

export interface AnalysisAccount {
  id: string;
  name: string;
  kind: string;
  balanceCents: number;
}

export interface MemberExpense {
  userId: string;
  displayName: string;
  expenseCents: number;
  transactionCount: number;
}

export interface AnalysisExpense {
  transactionId: string;
  description: string;
  submittedBy: string;
  amountCents: number;
  paidAt: string;
}
