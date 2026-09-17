/* Content filters belong to a checkout. The backend supplies every budget
 * from explorer_policy.json plus the saved settings overlay. */
let explorerPolicy = null;
let explorerFilterRoot = null;
let explorerFilters = emptyExplorerFilters();
const EXPLORER_FILTERS_KEY = 'zerocode.explorer-filters.v1';
const EXPLORER_PRESETS = ['src/**', 'tests/**', 'ui/**'];

function emptyExplorerFilters() {
  return { case_sensitive: false, whole_word: false, regex: false, include: '', exclude: '' };
}

function readExplorerFilters() {
  try { return JSON.parse(localStorage.getItem(EXPLORER_FILTERS_KEY) ?? '{}') ?? {}; }
  catch { return {}; }
}

function rememberExplorerFilters() {
  if (!explorerFilterRoot) explorerFilterRoot = activeWorktreePath;
  if (!explorerFilterRoot) return;
  try {
    const saved = readExplorerFilters();
    saved[explorerFilterRoot] = { ...explorerFilters };
    localStorage.setItem(EXPLORER_FILTERS_KEY, JSON.stringify(saved));
  } catch { /* Storage refusal keeps the current filter for this session. */ }
}

function restoreExplorerWorktree(root = activeWorktreePath) {
  if (explorerFilterRoot === root) return;
  ++fileSearchRequest;
  clearTimeout(fileSearchTimer);
  explorerFilterRoot = root;
  const saved = readExplorerFilters()[root] ?? {};
  explorerFilters = emptyExplorerFilters();
  for (const key of Object.keys(explorerFilters)) {
    if (typeof saved[key] === typeof explorerFilters[key]) explorerFilters[key] = saved[key];
  }
  paintExplorerFilters();
  resetTreeSelection();
  el("tree-undo-toast").hidden = true;
}

function resetExplorerFilters() {
  explorerFilters = emptyExplorerFilters();
  rememberExplorerFilters();
  paintExplorerFilters();
}

function paintExplorerFilters() {
  const row = el("file-search-filters");
  row.hidden = fileSearchMode !== 'text';
  const titles = {
    case_sensitive: t("file.searchCase", "대소문자 구분"),
    whole_word: t("file.searchWord", "단어 단위"),
    regex: t("file.searchRegex", "정규식"),
  };
  for (const [key, title] of Object.entries(titles)) {
    const button = el(`file-filter-${key === 'case_sensitive' ? 'case' : key === 'whole_word' ? 'word' : key}`);
    button.classList.toggle('is-active', explorerFilters[key]);
    button.setAttribute('aria-pressed', String(explorerFilters[key]));
    button.setAttribute('aria-label', title);
    button.dataset.tip = title;
  }
  el("file-filter-word").textContent = t("file.searchWordShort", "단어");
  for (const key of ['include', 'exclude']) {
    const input = el(`file-filter-${key}`);
    input.value = explorerFilters[key];
    const label = key === 'include' ? t("file.searchInclude", "포함할 경로") : t("file.searchExclude", "제외할 경로");
    input.placeholder = label;
    input.setAttribute('aria-label', label);
  }
  el("file-filter-presets").textContent = t("file.searchPresets", "프리셋");
  el("file-filter-reset").textContent = t("file.searchReset", "필터 초기화");
  el("file-search-error").hidden = true;
}

function explorerSearchOptions() {
  return Object.values(explorerFilters).some(Boolean) ? { ...explorerFilters } : null;
}

function explorerSearchError(error) {
  const messages = {
    'file.searchInvalidRegex': t("file.searchInvalidRegex", "정규식이 올바르지 않습니다. 괄호와 반복 기호를 확인하세요."),
    'file.searchInvalidGlob': t("file.searchInvalidGlob", "경로 패턴이 올바르지 않습니다. 와일드카드와 괄호를 확인하세요."),
    'file.searchTimeout': t("file.searchTimeout", "검색 시간 한도에 도달했습니다. 포함할 경로를 좁혀 주세요."),
  };
  el("file-search-error").textContent = messages[String(error)] ?? t("file.searchFailed", "검색하지 못했습니다. 경로와 필터를 확인하세요.");
  el("file-search-error").hidden = false;
}

function scheduleFileSearch() {
  ++fileSearchRequest;
  clearTimeout(fileSearchTimer);
  if (!explorerPolicy) return;
  fileSearchTimer = setTimeout(runFileSearch, fileSearchMode === 'name' ? explorerPolicy.name_debounce_ms : explorerPolicy.text_debounce_ms);
}

for (const [id, key] of [['case', 'case_sensitive'], ['word', 'whole_word'], ['regex', 'regex']]) {
  el(`file-filter-${id}`).addEventListener('click', () => {
    explorerFilters[key] = !explorerFilters[key];
    rememberExplorerFilters();
    paintExplorerFilters();
    void runFileSearch();
  });
}
for (const key of ['include', 'exclude']) {
  el(`file-filter-${key}`).addEventListener('input', (event) => {
    explorerFilters[key] = event.target.value;
    rememberExplorerFilters();
    scheduleFileSearch();
  });
}
el("file-filter-presets").addEventListener('click', () => {
  const popover = el("file-filter-preset-list");
  popover.hidden = !popover.hidden;
  if (!popover.hidden) popover.querySelector('button')?.focus();
});
el("file-filter-preset-list").addEventListener('keydown', (event) => {
  if (event.key !== 'Escape') return;
  event.stopPropagation();
  el("file-filter-preset-list").hidden = true;
  el("file-filter-presets").focus();
});
for (const pattern of EXPLORER_PRESETS) {
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = pattern;
  button.addEventListener('click', () => {
    explorerFilters.include = pattern;
    rememberExplorerFilters();
    paintExplorerFilters();
    el("file-filter-preset-list").hidden = true;
    fileSearch.focus();
    void runFileSearch();
  });
  el("file-filter-preset-list").appendChild(button);
}
el("file-filter-reset").addEventListener('click', () => { resetExplorerFilters(); void runFileSearch(); });
