import { openReactDialog } from "../components/openReactDialog";
import { type Transaction } from "../api";
import { TransactionForm } from "./TransactionForm";

/**
 * Add or edit a transaction. An `existing` generated occurrence of a repeating transaction edits
 * the series from that occurrence's month on; earlier months are left as they were.
 */
export function openTransactionDialog(existing?: Transaction, startAsIncome = false) {
  openReactDialog((close) => <TransactionForm existing={existing} startAsIncome={startAsIncome} close={close} />, "fullpage");
}
