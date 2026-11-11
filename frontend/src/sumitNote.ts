/**
 * An imported Sumit receipt keeps its products in the note: "קבלה 12 – customer: product | product [sumit:id]".
 * Returns the products (shown as badges) and the note without them; other notes are returned unchanged.
 */
export function splitSumitNote(note: string): { text: string; products: string[] } {
  const m = /^(.*?): (.+?) (\[sumit:\d+\])$/.exec(note);
  if (!m) return { text: note, products: [] };
  return { text: `${m[1]} ${m[3]}`.replace(/ \[sumit:\d+\]$/, ""), products: m[2]!.split(" | ").map((p) => p.trim()).filter(Boolean) };
}
