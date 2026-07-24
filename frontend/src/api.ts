import { invoke } from "@tauri-apps/api/core";

export type LedgerKind = "personal" | "organization_public";
export type TransactionKind = "income" | "expense" | "transfer";
export type ApprovalState = "draft" | "submitted" | "approved" | "rejected" | "voided";

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

export interface TransactionDto {
  id: string;
  ledgerId: string;
  accountId: string;
  kind: TransactionKind;
  amountMinor: number;
  currency: string;
  description: string;
  approvalState: ApprovalState;
  createdBy: string;
  createdByUserId: string;
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
  transactions: TransactionDto[];
  auditLogs: AuditLogDto[];
  monthlyIncomeMinor: number;
  monthlyExpenseMinor: number;
  pendingApprovalCount: number;
}

export interface CreateTransactionInput {
  ledgerId: string;
  accountId: string;
  kind: TransactionKind;
  amountMinor: number;
  currency: string;
  description: string;
}

export interface DecideApprovalInput {
  transactionId: string;
  decision: "approve" | "reject";
  decisionNote?: string;
}

export async function loadOverview(): Promise<LedgerOverview> {
  return invoke<LedgerOverview>("get_overview");
}

export async function createTransaction(
  input: CreateTransactionInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("create_transaction", { input });
}

export async function decideApproval(
  input: DecideApprovalInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("decide_approval", { input });
}
