/** The host builds the wording of confirmations; an empty quoted name («») it leaves behind is dropped on the way to the screen.
 *  Paths are shown as they are on the disk: nothing is translated or invented. */
export function tidyHuman(text: string): string {
  return text.replace(/\s*«»/g, '');
}
