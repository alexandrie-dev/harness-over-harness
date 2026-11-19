// Real-protocol shape: humanText is one line, with filenames separated by commas.
export const deletionText = 'Выполнить команду в папке Документы/HoH: удалить 3 старых файла: отчёт-2023.txt, черновик.docx, итог-старый.md';
export const approvalRequest = (extra = {}) => ({
  approvalId: 'approval-1', risk: 'high', humanText: deletionText,
  reason: 'Эти файлы старше года, а место в папке почти закончилось.',
  expiresAt: Date.now() + 5 * 60_000, ...extra,
});
