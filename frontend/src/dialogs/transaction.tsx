import { openReactDialog } from "../components/openReactDialog";
import { type Transaction } from "../api";
import { type Prefill, TransactionForm } from "./TransactionForm";

/**
 * Add or edit a transaction. An `existing` generated occurrence of a repeating transaction edits
 * the series from that occurrence's month on; earlier months are left as they were.
 * `prefill` starts a new transaction from an emailed suggestion; saving it marks the suggestion accepted.
 */
export function openTransactionDialog(existing?: Transaction, startAsIncome = false, prefill?: Prefill) {
  openReactDialog((close) => <TransactionForm existing={existing} startAsIncome={startAsIncome} prefill={prefill} close={close} />, "fullpage");
}
