export interface Transaction {
  id: number;
  date: string;
  amount_agorot: number;
  category_id: number | null;
  payment_method_id: number | null;
  note: string;
  sync_state: string;
}

export interface Category {
  id: number;
  name: string;
  is_income: boolean;
}

export interface MonthlyTotal {
  month: string;
  category_id: number | null;
  total_agorot: number;
  count: number;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { headers: { "Content-Type": "application/json" }, ...init });
  if (!res.ok) throw new Error(await res.text());
  return res.status === 204 ? (undefined as T) : res.json();
}

export const api = {
  transactions: () => request<Transaction[]>("/api/transactions"),
  categories: () => request<Category[]>("/api/categories"),
  monthlyTotals: () => request<MonthlyTotal[]>("/api/monthly-totals"),
  addTransaction: (t: Omit<Transaction, "id" | "sync_state">) =>
    request<number>("/api/transactions", { method: "POST", body: JSON.stringify(t) }),
  deleteTransaction: (id: number) => request<void>(`/api/transactions/${id}`, { method: "DELETE" }),
};

/** Parse a user-typed shekel amount ("12.5") into exact agorot. */
export function parseAgorot(input: string): number {
  const n = Number(input.replace(",", "."));
  if (!Number.isFinite(n)) throw new Error("Invalid amount");
  return Math.round(n * 100);
}

export function formatShekels(agorot: number): string {
  return (agorot / 100).toLocaleString(undefined, { style: "currency", currency: "ILS" });
}
