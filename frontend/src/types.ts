export type LedgerKind = "private" | "organization";

export type MemberRole =
  | "owner"
  | "admin"
  | "accountant"
  | "approver"
  | "member"
  | "auditor"
  | "viewer";

export type TransactionDirection = "expense" | "income";

export type ApprovalState = "draft" | "pending" | "approved" | "rejected" | "deleted";

export type CloudConnectionState = "checking" | "online" | "offline";

export type AuditAction =
  | "transaction_created"
  | "transaction_submitted"
  | "transaction_approved"
  | "transaction_rejected"
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
}

export interface CloudStatus {
  state: CloudConnectionState;
  label: string;
  detail?: string;
}

export interface UserSession {
  currentUser: UserAccount;
  users: UserAccount[];
  cloudStatus: CloudStatus;
}

export interface FinancialAccount {
  id: string;
  ledgerId: string;
  name: string;
  kind: "cash" | "bank" | "wallet" | "company" | "receivable" | "payable";
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
  actorName: string;
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

export interface ApprovalQueueItem {
  id: string;
  transactionId: string;
  ledgerId: string;
  title: string;
  amountCents: number;
  direction: TransactionDirection;
  submittedBy: string;
  submittedAt: string;
  state: Extract<ApprovalState, "pending" | "rejected">;
}

export interface AuditLogEntry {
  id: string;
  ledgerId: string;
  action: AuditAction;
  actorName: string;
  createdAt: string;
  summary: string;
}

export interface LedgerDashboard {
  ledger: Ledger;
  accounts: FinancialAccount[];
  categories: Category[];
  recentTransactions: Transaction[];
  approvalQueue: ApprovalQueueItem[];
  auditTrail: AuditLogEntry[];
}
