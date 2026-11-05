import { formatShekels, formatShekelsWhole } from "../format";

/** A shekel amount (given in agorot), coloured by `cls` ("expense" or "income"); `whole` rounds to whole shekels, as the dashboards show it. */
export function Money({ agorot, cls = "", whole = false }: { agorot: number; cls?: string; whole?: boolean }) {
  return <span className={`num ${cls}`.trim()}>{(whole ? formatShekelsWhole : formatShekels)(agorot)}</span>;
}
