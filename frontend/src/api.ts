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
  occurredAt: string;
}

export interface AuditLogDto {
  id: string;
  ledgerId: string;
  actorUserId: string;
  action: string;
  resourceType: string;
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
  actorUserId: string;
  ledgerId: string;
  accountId: string;
  kind: TransactionKind;
  amountMinor: number;
  currency: string;
  description: string;
}

export async function loadOverview(): Promise<LedgerOverview> {
  try {
    return await invoke<LedgerOverview>("get_overview");
  } catch {
    return demoOverview();
  }
}

export async function createTransaction(
  input: CreateTransactionInput
): Promise<TransactionDto> {
  return invoke<TransactionDto>("create_transaction", { input });
}

function demoOverview(): LedgerOverview {
  const userId = "demo-user";
  const personalLedgerId = "personal-ledger";
  const companyLedgerId = "company-ledger";
  const personalAccountId = "personal-cash";
  const companyAccountId = "company-bank";

  return {
    currentUser: { id: userId, displayName: "Alice" },
    ledgers: [
      {
        id: personalLedgerId,
        name: "Alice 私账",
        kind: "personal",
        scopeLabel: "私账"
      },
      {
        id: companyLedgerId,
        name: "Acme 公账",
        kind: "organization_public",
        scopeLabel: "公账",
        organizationId: "acme"
      }
    ],
    accounts: [
      {
        id: personalAccountId,
        ledgerId: personalLedgerId,
        name: "个人现金",
        kind: "cash",
        balanceMinor: 338000,
        currency: "CNY"
      },
      {
        id: companyAccountId,
        ledgerId: companyLedgerId,
        name: "公司银行账户",
        kind: "bank",
        balanceMinor: 4914000,
        currency: "CNY"
      }
    ],
    transactions: [
      {
        id: "salary",
        ledgerId: personalLedgerId,
        accountId: personalAccountId,
        kind: "income",
        amountMinor: 1800000,
        currency: "CNY",
        description: "工资收入",
        approvalState: "approved",
        createdBy: userId,
        occurredAt: new Date().toISOString()
      },
      {
        id: "office",
        ledgerId: companyLedgerId,
        accountId: companyAccountId,
        kind: "expense",
        amountMinor: 86000,
        currency: "CNY",
        description: "办公用品采购",
        approvalState: "submitted",
        createdBy: userId,
        occurredAt: new Date().toISOString()
      }
    ],
    auditLogs: [
      {
        id: "audit-office",
        ledgerId: companyLedgerId,
        actorUserId: userId,
        action: "transaction.submitted",
        resourceType: "transaction",
        summary: "提交公账支出：办公用品采购",
        createdAt: new Date().toISOString()
      }
    ],
    monthlyIncomeMinor: 1800000,
    monthlyExpenseMinor: 86000,
    pendingApprovalCount: 1
  };
}
