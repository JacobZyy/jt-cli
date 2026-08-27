export function unusedExport(): void {}

export function importedButUnread(): void {}

export function recursiveOnly(): void {
  recursiveOnly()
}

export let writeOnly: number
writeOnly = 1
