import { formatShekels } from "../format";

/** A shekel amount (given in agorot), coloured by `cls` ("expense" or "income"). */
export function Money({ agorot, cls = "" }: { agorot: number; cls?: string }) {
  return <span className={`num ${cls}`.trim()}>{formatShekels(agorot)}</span>;
}
