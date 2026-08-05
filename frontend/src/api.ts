import { invoke } from "@tauri-apps/api/core";

export type LedgerKind = "personal" | "organization_public";
export type TransactionKind = "income" | "expense" | "transfer";
export type ApprovalState = "draft" | "submitted" | "approved" | "rejected" | "voided";
export type PaymentState = "not_applicable" | "pending_payment" | "paid_pending_receipt" | "received";

export interface UserDto {
  id: string;
  displayName: string;
}

export interface LedgerDto {
  id: string;
  name: string;
  kind: LedgerKind;
  scopeLabel: string;
  organizationId?: string;
  role: string;
}

export interface AccountDto {
  id: string;
  ledgerId: string;
  name: string;
  kind: string;
  balanceMinor: number;
  currency: string;
}

export interface CategoryDto {
  id: string;
  ledgerId: string;
  name: string;
  kind: "income" | "expense";
}

export interface TransactionDto {
  id: string;
  ledgerId: string;
  accountId: string;
  categoryId?: string;
  kind: TransactionKind;
  amountMinor: number;
  currency: string;
  description: string;
  approvalState: ApprovalState;
  paymentState: PaymentState;
  createdBy: string;
  createdByUserId: string;
  approvedAt?: string;
  paidAt?: string;
  receivedAt?: string;
  occurredAt: string;
}

export interface AuditLogDto {
  id: string;
  ledgerId: string;
  actorUserId: string;
  actorDisplayName: string;
  action: string;
  resourceType: string;
  resourceId: string;
  summary: string;
  createdAt: string;
}

export interface LedgerOverview {
  currentUser: UserDto;
  ledgers: LedgerDto[];
  accounts: AccountDto[];
  categories: CategoryDto[];
  transactions: TransactionDto[];
  auditLogs: AuditLogDto[];
  monthlyIncomeMinor: number;
  monthlyExpenseMinor: number;
  pendingApprovalCount: number;
  pendingPaymentCount: number;
  pendingReceiptCount: number;
}

export interface TransactionMonthDto {
  ledgerId: string;
  month: string;
  availableMonths: string[];
  transactions: TransactionDto[];
}

export interface FinancialAnalysisDto {
  ledgerId: string;
  currency: string;
  months: number;
  periodStart: string;
  periodEnd: string;
  currentBalanceMinor: number;
  incomeMinor: number;
  expenseMinor: number;
  netCashFlowMinor: number;
  previousIncomeMinor: number;
  previousExpenseMinor: number;
  previousNetCashFlowMinor: number;
  transactionCount: number;
  pendingApproval: FinancialExposureDto;
  pendingPayment: FinancialExposureDto;
  paidPendingReceipt: FinancialExposureDto;
  trend: CashFlowTrendPointDto[];
  accounts: AnalysisAccountDto[];
  memberExpenses: MemberExpenseDto[];
  largestExpenses: AnalysisExpenseDto[];
  generatedAt: string;
}

export interface FinancialExposureDto {
  count: number;
  amountMinor: number;
}

export interface CashFlowTrendPointDto {
  key: string;
  label: string;
  incomeMinor: number;
  expenseMinor: number;
  netCashFlowMinor: number;
}

export interface AnalysisAccountDto {
  id: string;
  name: string;
  kind: string;
  balanceMinor: number;
}

export interface MemberExpenseDto {
  userId: string;
  displayName: string;
  expenseMinor: number;
  transactionCount: number;
}

export interface AnalysisExpenseDto {
  transactionId: string;
  description: string;
  submittedBy: string;
  amountMinor: number;
  paidAt: string;
}

export interface CreateTransactionInput {
  ledgerId: string;
  accountId: string;
  categoryId: string;
  kind: TransactionKind;
  amountMinor: number;
  currency: string;
  description: string;
  clientMutationId?: string;
}

export interface CreateCategoryInput {
  ledgerId: string;
  name: string;
  kind: "income" | "expense";
}

export interface DecideApprovalInput {
  transactionId: string;
  decision: "approve" | "reject";
  decisionNote?: string;
}

export interface TransactionActionInput {
  transactionId: string;
}

export async function loadOverview(): Promise<LedgerOverview> {
  return invoke<LedgerOverview>("get_overview");
}

export async function loadFinancialAnalysis(
  ledgerId: string,
  months: number,
): Promise<FinancialAnalysisDto> {
  return invoke<FinancialAnalysisDto>("get_financial_analysis", { ledgerId, months });
}

export async function createTransaction(
  input: CreateTransactionInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("create_transaction", { input });
}

export async function loadTransactionsForMonth(
  ledgerId: string,
  month?: string,
): Promise<TransactionMonthDto> {
  return invoke<TransactionMonthDto>("get_transactions_for_month", { ledgerId, month });
}

export async function createCategory(input: CreateCategoryInput): Promise<CategoryDto> {
  return invoke<CategoryDto>("create_category", { input });
}

export async function decideApproval(
  input: DecideApprovalInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("decide_approval", { input });
}

export async function markTransactionPaid(
  input: TransactionActionInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("mark_transaction_paid", { input });
}

export async function confirmTransactionReceipt(
  input: TransactionActionInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("confirm_transaction_receipt", { input });
}
