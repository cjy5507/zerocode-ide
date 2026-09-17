// The bench's clipboard keeper (docs/design/computer-use-bench.md §1): every
// item and every type on a pasteboard saved to files, the pasteboard cleared,
// and the same items put back — text, a picture, copied files alike.
//
//   osascript -l JavaScript pasteboard.js save|restore|clear DIR [NAME]
//
// NAME picks a named pasteboard instead of the general one (the tests use
// one, so they never touch the person's clipboard). `save` writes
// DIR/manifest.json and one file per type; `restore` reads them back.
ObjC.import('AppKit');

function board(name) {
  return name ? $.NSPasteboard.pasteboardWithName(name) : $.NSPasteboard.generalPasteboard;
}

function save(pasteboard, dir) {
  const items = pasteboard.pasteboardItems;
  const saved = [];
  const count = items.isNil() ? 0 : items.count;
  for (let i = 0; i < count; i++) {
    const item = items.objectAtIndex(i);
    const types = item.types;
    const entry = [];
    for (let j = 0; j < types.count; j++) {
      const type = types.objectAtIndex(j);
      const data = item.dataForType(type);
      if (data.isNil()) continue;
      const file = `${i}-${j}.bin`;
      if (!data.writeToFileAtomically(`${dir}/${file}`, true)) throw new Error(`could not write ${file}`);
      entry.push({ type: ObjC.unwrap(type), file });
    }
    saved.push(entry);
  }
  const manifest = $.NSString.alloc.initWithUTF8String(JSON.stringify({ items: saved }));
  if (!manifest.writeToFileAtomicallyEncodingError(`${dir}/manifest.json`, true, $.NSUTF8StringEncoding, null)) {
    throw new Error('could not write the manifest');
  }
  return saved.length;
}

function restore(pasteboard, dir) {
  const text = $.NSString.stringWithContentsOfFileEncodingError(`${dir}/manifest.json`, $.NSUTF8StringEncoding, null);
  if (text.isNil()) throw new Error(`no manifest in ${dir}`);
  const objects = JSON.parse(ObjC.unwrap(text)).items.map((entry) => {
    const item = $.NSPasteboardItem.alloc.init;
    for (const { type, file } of entry) {
      const data = $.NSData.dataWithContentsOfFile(`${dir}/${file}`);
      if (data.isNil()) throw new Error(`missing ${file}`);
      item.setDataForType(data, type);
    }
    return item;
  });
  pasteboard.clearContents;
  if (objects.length && !pasteboard.writeObjects($(objects))) throw new Error('the pasteboard refused the items');
  return objects.length;
}

function run(argv) {
  const [mode, dir, name] = argv;
  const pasteboard = board(name);
  if (mode === 'clear') {
    pasteboard.clearContents;
    return 0;
  }
  if (mode === 'save' && dir) return save(pasteboard, dir);
  if (mode === 'restore' && dir) return restore(pasteboard, dir);
  throw new Error('usage: save|restore|clear DIR [NAME]');
}
