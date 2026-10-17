import '@angular/compiler';
import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import {
  ElementRef,
  EnvironmentInjector,
  Injector,
  computed,
  createEnvironmentInjector,
  runInInjectionContext,
  signal,
  ɵChangeDetectionScheduler,
  ɵEffectScheduler,
} from '@angular/core';
import { HttpErrorResponse } from '@angular/common/http';
import { ActivatedRoute, convertToParamMap } from '@angular/router';
import { BehaviorSubject, Subject, of, throwError } from 'rxjs';
import {
  AddWordsResult,
  ScreenshotResult,
  Session,
  SessionRequest,
  WORD_BATCH_SIZE,
  WordDetail as WordDetailData,
  WordInput,
} from '@moonrabbit/contract';
import { AudioCache } from '../core/audio';
import { Api } from '../core/api';
import { Grades } from '../core/grades';
import { HomePage } from './home';
import { ListPage } from './list';
import { QueuePage } from './queue';
import { StudyPage } from './study';
import { WordImport } from './word-import';
import { WordDetail } from './word-detail';
import { WordExamples } from './word-examples';

const cleanups: (() => void)[] = [];
afterEach(() => {
  for (const cleanup of cleanups.splice(0).reverse()) cleanup();
});
const example = {
  simplified: '我有字。',
  pinyin: 'wǒ yǒu zì.',
  pronouncedPinyin: 'wó yǒu zì.',
  translation: 'I have a character.',
};
const word = {
  id: 'word',
  simplified: '字',
  pinyin: 'zì',
  pronouncedPinyin: 'zì',
  translation: 'character',
  studied: false,
  examples: [],
  cards: [],
};
const face = {
  wordId: word.id,
  direction: 'recognition' as const,
  simplified: word.simplified,
  pinyin: word.pinyin,
  translation: word.translation,
};
const list = {
  id: 'list',
  title: 'List',
  owner: 'local',
  words: 1,
  wordsLeft: 1,
  charactersStudied: 0,
  characterCount: 1,
};
const page = {
  id: list.id,
  title: list.title,
  owner: list.owner,
  words: [word],
  totalWords: 1,
  nextCursor: null,
};
const board = {
  due: 0,
  recallable: 0,
  forgotten: 0,
  charactersStudied: 0,
  retention: null,
  days: [],
  queue: [list],
  queueVersion: 3,
};

// This is the resource consumer seam, not a substitute implementation of HTTP cancellation.
function resource<T>(resolve: () => T | undefined) {
  const error = signal<unknown>(undefined);
  const isLoading = signal(false);
  const revision = signal(0);
  const replacement = signal<{ value: T } | undefined>(undefined);
  const data = computed(() => {
    revision();
    return replacement()?.value ?? resolve();
  });
  return {
    error,
    isLoading,
    status: computed(() =>
      error() ? 'error' : isLoading() ? 'loading' : data() === undefined ? 'idle' : 'resolved',
    ),
    hasValue: () => !error() && data() !== undefined,
    value: () => {
      if (error()) throw error();
      const value = data();
      if (value === undefined) throw new Error('Resource has no value');
      return value;
    },
    reload: () => {
      revision.update((value) => value + 1);
      return true;
    },
    set: (value: T) => replacement.set({ value }),
  };
}

function browserClock() {
  const keys = ['window', 'document', 'HTMLElement', 'FileReader'] as const;
  const descriptors = keys.map((key) => Object.getOwnPropertyDescriptor(globalThis, key));
  cleanups.push(() =>
    keys.forEach((key, index) => {
      const descriptor = descriptors[index];
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }),
  );
  let id = 0;
  const intervals = new Map<number, () => void>();
  const timeouts = new Map<number, () => void>();
  class ImageReader {
    static LOADING = 1;
    readyState = 0;
    result: string | null = null;
    error: Error | null = null;
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    onabort: (() => void) | null = null;
    readAsDataURL(file: { dataUrl?: string | Promise<string> }) {
      this.readyState = 1;
      Promise.resolve(file.dataUrl ?? 'data:image/png;base64,aW1hZ2U=').then((image) => {
        if (this.readyState !== 1) return;
        this.result = image;
        this.readyState = 2;
        this.onload?.();
      });
    }
    abort() {
      this.readyState = 2;
      this.onabort?.();
    }
  }
  Object.assign(globalThis, {
    window: {
      setInterval(callback: () => void) {
        intervals.set(++id, callback);
        return id;
      },
      clearInterval(timer: number) {
        intervals.delete(timer);
      },
      setTimeout(callback: () => void) {
        timeouts.set(++id, callback);
        return id;
      },
      clearTimeout(timer: number) {
        timeouts.delete(timer);
      },
    },
    document: { activeElement: null },
    HTMLElement: class {},
    FileReader: ImageReader,
  });
  return { intervals, timeouts };
}

function component<T>(type: new () => T, api: object = {}, grades: object = {}) {
  const route = new BehaviorSubject(convertToParamMap({ id: list.id }));
  let stops = 0;
  const effects = new Set<{ run(): void; dirty: boolean }>();
  const scheduler = {
    add(effect: { run(): void; dirty: boolean }) {
      effects.add(effect);
    },
    schedule(effect: { run(): void; dirty: boolean }) {
      effects.add(effect);
    },
    remove(effect: { run(): void; dirty: boolean }) {
      effects.delete(effect);
    },
    flush() {
      while (effects.size) {
        const pending = [...effects];
        effects.clear();
        for (const effect of pending) if (effect.dirty) effect.run();
      }
    },
  };
  const injector = createEnvironmentInjector(
    [
      { provide: ɵEffectScheduler, useValue: scheduler },
      { provide: ɵChangeDetectionScheduler, useValue: { notify: mock.fn(), runningTick: false } },
      {
        provide: Api,
        useValue: {
          dashboard: () => resource(() => board),
          lists: () => resource(() => [list, { ...list, id: 'A' }, { ...list, id: 'B' }]),
          search: () => resource(() => undefined),
          word: () => resource(() => undefined),
          session: (request: () => SessionRequest | undefined) =>
            resource(() => (request() ? { cards: [face] } : undefined)),
          list: (request: () => { id: string; cursor: number } | undefined) =>
            resource(() => {
              const current = request();
              return current ? { ...page, id: current.id } : undefined;
            }),
          ...api,
        },
      },
      { provide: Grades, useValue: grades },
      {
        provide: AudioCache,
        useValue: {
          stop() {
            stops++;
          },
          play: async () => undefined,
        },
      },
      { provide: ActivatedRoute, useValue: { paramMap: route } },
    ],
    Injector.NULL as EnvironmentInjector,
  );
  const instance = runInInjectionContext(injector, () => new type());
  let destroyed = false;
  const destroy = () => {
    if (!destroyed) {
      destroyed = true;
      injector.destroy();
    }
  };
  cleanups.push(destroy);
  if (instance instanceof WordImport) {
    instance.listId = list.id;
    instance.editable = true;
  }
  return {
    instance,
    route,
    injector,
    destroy,
    flushEffects: () => scheduler.flush(),
    stopCount: () => stops,
  };
}

function input(value: string): Event {
  return { target: { value } } as unknown as Event;
}
function imageEvent(dataUrl?: string | Promise<string>): Event {
  return {
    target: { files: [{ type: 'image/png', size: 100, dataUrl }], value: 'screenshot' },
  } as unknown as Event;
}
function screenshot(count = 1): ScreenshotResult {
  return {
    words: Array.from({ length: count }, () => ({
      simplified: '字',
      readings: [
        {
          pinyin: 'zì',
          pronouncedPinyin: 'zì',
          translation: 'character',
          examples: [{ ...example }],
        },
      ],
      matches: [{ ...word, lists: [] }],
    })),
    warnings: [],
  };
}

test('queue mutations are single-flight and use the loaded conflict version', () => {
  browserClock();
  const pending = new Subject<unknown>();
  const writes: { ids: string[]; version: number }[] = [];
  const fixture = component(QueuePage, {
    setQueue(ids: string[], version: number) {
      writes.push({ ids, version });
      return pending;
    },
  });
  fixture.instance.add('second');
  fixture.instance.remove(list.id);
  assert.deepEqual(writes, [{ ids: ['list', 'second'], version: 3 }]);
  pending.error({ status: 409 });
  assert.deepEqual(
    fixture.instance.queue().map((item) => item.id),
    ['list'],
  );
  assert.equal(fixture.instance.saving(), false);
  assert.match(fixture.instance.error(), /changed elsewhere/);
});

test('errored dashboard, list, queue, search and detail resources never read unavailable values', () => {
  browserClock();
  const failed = resource(() => undefined);
  failed.error.set(new Error('unavailable'));
  const api = {
    dashboard: () => failed,
    lists: () => failed,
    list: () => failed,
    word: () => failed,
    search: () => failed,
  };
  const home = component(HomePage, api).instance;
  const queue = component(QueuePage, api).instance;
  const lists = component(ListPage, api).instance;
  const editor = component(WordImport, api).instance;
  const detail = component(WordDetail, api).instance;
  assert.equal(home.data(), null);
  assert.deepEqual(queue.queue(), []);
  assert.equal(queue.busy(), true);
  assert.deepEqual(lists.words(), []);
  assert.deepEqual(editor.results(), []);
  assert.deepEqual(editor.personalLists(), []);
  assert.equal(detail.detail(), null);
  assert.ok(
    home.error() &&
      queue.loadError() &&
      lists.loadError() &&
      editor.searchError() &&
      detail.error(),
  );
});

test('launch captures one exclusive direction and destruction cancels starts and timers', () => {
  const clock = browserClock();
  let requested!: () => SessionRequest | undefined;
  const response = signal<Session | undefined>(undefined);
  const session = resource(() => (requested() ? response() : undefined));
  const fixture = component(StudyPage, {
    session(request: () => SessionRequest | undefined) {
      requested = request;
      return session;
    },
  });
  fixture.instance.unit.set('minutes');
  fixture.instance.direction.set('production');
  fixture.instance.start();
  fixture.instance.unit.set('words');
  fixture.instance.amount.set(20);
  fixture.instance.direction.set('recognition');
  fixture.instance.start();
  assert.deepEqual(requested(), { unit: 'minutes', amount: 2, direction: 'production' });
  fixture.flushEffects();
  assert.equal(fixture.instance.launching(), true);
  response.set({ cards: [face] });
  fixture.flushEffects();
  assert.equal(fixture.instance.sessionUnit, 'minutes');
  assert.equal(fixture.instance.remaining(), 120);
  assert.equal(clock.intervals.size, 1);
  fixture.destroy();
  assert.equal(clock.intervals.size, 0);
  assert.ok(fixture.stopCount() > 0);
});

test('storage failure cannot count or advance an answer; double answers are blocked', async () => {
  browserClock();
  let writes = 0;
  let delivery = Promise.withResolvers<void>();
  const fixture = component(
    StudyPage,
    {
      session: (request: () => SessionRequest | undefined) =>
        resource(() => (request() ? { cards: [face, face] } : undefined)),
    },
    {
      enqueue() {
        writes++;
        return delivery.promise;
      },
    },
  );
  fixture.instance.start();
  fixture.flushEffects();
  await fixture.instance.know(true);
  const first = fixture.instance.correct(true);
  await fixture.instance.correct(false);
  assert.equal(writes, 1);
  assert.equal(fixture.instance.good(), 0);
  delivery.reject(new Error('Quota exceeded'));
  await first;
  assert.equal(fixture.instance.phase(), 'reveal');
  assert.equal(fixture.instance.index(), 0);
  delivery = Promise.withResolvers<void>();
  const retry = fixture.instance.correct(true);
  delivery.resolve();
  await retry;
  fixture.flushEffects();
  assert.equal(fixture.instance.good(), 1);
  assert.equal(fixture.instance.index(), 1);
});

test('timed expiry and explicit end never grade the unanswered current card', () => {
  const clock = browserClock();
  let writes = 0;
  const fixture = component(
    StudyPage,
    {},
    {
      enqueue: async () => {
        writes++;
      },
    },
  );
  fixture.instance.unit.set('minutes');
  fixture.instance.start();
  fixture.flushEffects();
  fixture.instance.deadline = 0;
  for (const tick of clock.intervals.values()) tick();
  assert.equal(fixture.instance.phase(), 'summary');
  assert.equal(clock.intervals.size, 0);
  assert.equal(writes, 0);
});

test('revealed study examples request only the active word, never the question side', async () => {
  browserClock();
  let requested!: () => string | undefined;
  const fixture = component(
    StudyPage,
    {
      session: (request: () => SessionRequest | undefined) =>
        resource(() => (request() ? { cards: [face, { ...face, wordId: 'next' }] } : undefined)),
      word: (request: () => string | undefined) => {
        requested = request;
        return resource(() => {
          const id = requested();
          return id ? { ...word, id, cards: [] } : undefined;
        });
      },
    },
    { enqueue: async () => undefined },
  );
  fixture.instance.start();
  fixture.flushEffects();
  assert.equal(requested(), undefined);
  await fixture.instance.know(true);
  assert.equal(requested(), word.id);
  await fixture.instance.correct(true);
  assert.equal(requested(), undefined);
  await fixture.instance.know(true);
  assert.equal(requested(), 'next');
});

test('cleared searches disable the resource and manual edits invalidate all AI-filled fields', () => {
  const clock = browserClock();
  let requested!: () => string | undefined;
  const fill = new Subject<unknown>();
  const fixture = component(WordImport, {
    search: (query: () => string | undefined) => {
      requested = query;
      return resource(() => (query() ? [{ ...word, lists: [list] }] : undefined));
    },
    drafts: () => fill,
  });
  fixture.instance.search(input('字'));
  for (const callback of clock.timeouts.values()) callback();
  assert.equal(requested(), '字');
  assert.equal(fixture.instance.results()[0].lists[0].id, list.id);
  fixture.instance.search(input(''));
  assert.equal(requested(), undefined);
  assert.deepEqual(fixture.instance.results(), []);
  fixture.instance.typed('simplified', input('字'));
  fixture.instance.fill();
  fixture.instance.typed('pronouncedPinyin', input('manual'));
  fill.next([
    {
      simplified: '字',
      readings: [{ pinyin: 'zì', pronouncedPinyin: 'zì', translation: 'character' }],
    },
  ]);
  assert.equal(fixture.instance.state().pronouncedPinyin(), 'manual');
  assert.equal(fixture.instance.state().pinyin(), '');
});

test('AI pronounced pinyin is editable and persists in manual word writes', () => {
  browserClock();
  const writes: WordInput[][] = [];
  const fixture = component(WordImport, {
    drafts: () =>
      of([
        {
          simplified: '字',
          readings: [{ pinyin: 'zì', pronouncedPinyin: 'zì', translation: 'character' }],
        },
      ]),
    addWords: (_id: string, words: WordInput[]) => {
      writes.push(words);
      return of({ added: 1, rejected: [] });
    },
  });
  fixture.instance.typed('simplified', input('字'));
  fixture.instance.fill();
  assert.equal(fixture.instance.state().pronouncedPinyin(), 'zì');
  fixture.instance.typed('pronouncedPinyin', input('zì edited'));
  fixture.instance.saveManual();
  assert.equal(writes[0][0].pronouncedPinyin, 'zì edited');
});

test('closed details disable reads and restore focus; Tab remains contained', () => {
  browserClock();
  let requested!: () => string | undefined;
  const fixture = component(WordDetail, {
    word: (request: () => string | undefined) => {
      requested = request;
      return resource(() => (request() ? { ...word, cards: [] } : undefined));
    },
  });
  const opener = new HTMLElement();
  let restored = 0;
  opener.focus = () => {
    restored++;
  };
  Object.assign(document, { activeElement: opener });
  let focused = '';
  const first = {
    getClientRects: () => [1],
    focus: () => {
      focused = 'first';
    },
  };
  const last = {
    getClientRects: () => [1],
    focus: () => {
      focused = 'last';
    },
  };
  const dialog = {
    open: false,
    showModal() {
      this.open = true;
    },
    close() {
      this.open = false;
    },
    querySelectorAll: () => [first, last],
  };
  fixture.instance.dialog = new ElementRef(dialog as unknown as HTMLDialogElement);
  fixture.instance.open(word);
  assert.equal(requested(), word.id);
  Object.assign(document, { activeElement: last });
  let prevented = false;
  fixture.instance.containFocus({
    key: 'Tab',
    shiftKey: false,
    preventDefault() {
      prevented = true;
    },
  } as KeyboardEvent);
  assert.equal(prevented, true);
  assert.equal(focused, 'first');
  Object.assign(document, { activeElement: first });
  prevented = false;
  fixture.instance.containFocus({
    key: 'Tab',
    shiftKey: true,
    preventDefault() {
      prevented = true;
    },
  } as KeyboardEvent);
  assert.equal(prevented, true);
  assert.equal(focused, 'last');
  fixture.instance.close();
  assert.equal(requested(), undefined);
  assert.equal(fixture.instance.detail(), null);
  assert.equal(dialog.open, false);
  assert.equal(restored, 1);
});

test('route and pagination changes drive resource parameters; removing the final row returns a page', () => {
  browserClock();
  let requested!: () => { id: string; cursor: number } | undefined;
  let total = 101;
  const fixture = component(ListPage, {
    list: (request: typeof requested) => {
      requested = request;
      return resource(() => {
        const current = request();
        return current
          ? {
              ...page,
              id: current.id,
              title: current.id,
              totalWords: total,
              words: Array(Math.min(100, total - current.cursor)).fill(word),
              nextCursor: current.cursor + 100 < total ? current.cursor + 100 : null,
            }
          : undefined;
      });
    },
    removeWord: () => {
      total = 100;
      return of({});
    },
  });
  assert.equal(fixture.instance.words().length, 100);
  fixture.instance.load(100);
  assert.deepEqual(requested(), { id: 'list', cursor: 100 });
  assert.equal(fixture.instance.words().length, 1);
  fixture.instance.remove(word.id);
  assert.equal(fixture.instance.cursor(), 0);
  assert.equal(fixture.instance.words().length, 100);
  fixture.route.next(convertToParamMap({ id: 'new' }));
  assert.deepEqual(requested(), { id: 'new', cursor: 0 });
  assert.equal(fixture.instance.title(), 'new');
});

test('screenshot previews retain every reading, dictionary memberships and independently editable source sentences', async () => {
  browserClock();
  const result = screenshot(41);
  result.words[0].readings.push({
    pinyin: 'zǐ',
    pronouncedPinyin: 'zí',
    translation: 'alternate',
    examples: [{ ...example }],
  });
  result.words[1].matches = [];
  result.words[2].matches[0].lists = [{ id: 'hsk-1', title: 'HSK 1' }];
  const fixture = component(WordImport, { screenshot: () => of(result) });
  await fixture.instance.onFile(imageEvent());
  const rows = fixture.instance.state().drafts();
  assert.equal(rows.length, 42);
  assert.equal(fixture.instance.state().visibleDrafts().length, WORD_BATCH_SIZE);
  assert.equal(new Set(rows.map((row) => row.id)).size, 42);
  assert.equal(rows[0].matches[0].lists.length, 0);
  assert.equal(rows[2].matches.length, 0);
  assert.equal(rows[3].matches[0].lists[0].title, 'HSK 1');
  fixture.instance.setDraft(rows[0], 'pronouncedPinyin', input('edited'));
  fixture.instance.setExample(rows[0], rows[0].examples[0], 'translation', input('my sentence'));
  assert.equal(rows[0].pronouncedPinyin, 'edited');
  assert.equal(rows[0].examples[0].translation, 'my sentence');
  assert.equal(result.words[0].readings[0].examples?.[0].translation, example.translation);
});

test('batched screenshot saves preserve sentence edits and retain only rejected and unconfirmed rows', async () => {
  browserClock();
  const writes: WordInput[][] = [];
  const fixture = component(WordImport, {
    screenshot: () => of(screenshot(41)),
    addWords: (_id: string, words: WordInput[]) => {
      writes.push(words);
      return writes.length === 1
        ? of({ added: 39, rejected: [{ index: 4, error: 'Correct this reading' }] })
        : throwError(() => new Error('transport failure'));
    },
  });
  await fixture.instance.onFile(imageEvent());
  const original = fixture.instance.state().drafts().slice();
  fixture.instance.setExample(
    original[0],
    original[0].examples[0],
    'pronouncedPinyin',
    input('edited sentence pronunciation'),
  );
  await fixture.instance.saveDrafts();
  assert.deepEqual(
    writes.map((batch) => batch.length),
    [40, 1],
  );
  assert.equal(writes[0][0].examples?.[0].pronouncedPinyin, 'edited sentence pronunciation');
  assert.deepEqual(
    fixture.instance
      .state()
      .drafts()
      .map((row) => row.id),
    [original[4].id, original[40].id],
  );
  assert.equal(original[4].errors.list, 'Correct this reading');
  assert.ok(fixture.instance.state().saveErrors().list);
});

test('sentence-heavy screenshot previews stay below the HTTP body limit and save every row', async () => {
  browserClock();
  const result = screenshot(8);
  for (const row of result.words) {
    row.readings[0].examples = Array.from({ length: 20 }, (_, index) => ({
      simplified: '字'.repeat(498) + String(index).padStart(2, '0'),
      pinyin: 'zì '.repeat(499).trim(),
      pronouncedPinyin: 'zì '.repeat(499).trim(),
      translation: 't'.repeat(2000),
    }));
  }
  const encoder = new TextEncoder();
  const fixture = component(WordImport, {
    screenshot: () => of(result),
    addWords: (_id: string, words: WordInput[]) =>
      encoder.encode(JSON.stringify({ words })).byteLength > 1_048_576
        ? throwError(() => new HttpErrorResponse({ status: 413 }))
        : of({ added: words.length, rejected: [] }),
  });
  await fixture.instance.onFile(imageEvent());
  await fixture.instance.saveDrafts();
  assert.deepEqual(fixture.instance.state().drafts(), []);
  assert.equal(fixture.instance.state().saveErrors().list, '');
});

test('partial screenshot saves and errors stay with their captured destination across reused routes', async () => {
  browserClock();
  const second = new Subject<AddWordsResult>();
  const writes: { listId: string; count: number }[] = [];
  const fixture = component(WordImport, {
    screenshot: () => of(screenshot(41)),
    addWords: (listId: string, words: WordInput[]) => {
      writes.push({ listId, count: words.length });
      return writes.length === 2 ? second : of({ added: words.length, rejected: [] });
    },
  });
  fixture.instance.listId = 'A';
  fixture.instance.state().destination.set('B');
  await fixture.instance.onFile(imageEvent());
  const saving = fixture.instance.saveDrafts();
  await Promise.resolve();
  assert.deepEqual(writes, [
    { listId: 'B', count: 40 },
    { listId: 'B', count: 1 },
  ]);
  fixture.instance.listId = 'B';
  fixture.instance.typed('simplified', input('B edit'));
  second.error(new Error('transport failure'));
  await saving;
  assert.equal(fixture.instance.state().error(), '');
  assert.equal(fixture.instance.state().simplified(), 'B edit');
  assert.deepEqual(fixture.instance.state().drafts(), []);
  fixture.instance.listId = 'A';
  assert.equal(fixture.instance.destination(), 'B');
  assert.equal(fixture.instance.state().drafts().length, 1);
  assert.ok(fixture.instance.state().saveErrors().B);
  fixture.instance.state().destination.set('A');
  assert.equal(fixture.instance.state().saveErrors().A, undefined);
  fixture.instance.state().destination.set('B');
  await fixture.instance.saveDrafts();
  assert.deepEqual(writes[2], { listId: 'B', count: 1 });
  assert.deepEqual(fixture.instance.state().drafts(), []);
});

test('file reading and screenshot provider completion retain the originating preview after route reuse', async () => {
  browserClock();
  const file = Promise.withResolvers<string>();
  const provider = new Subject<ScreenshotResult>();
  const fixture = component(WordImport, { screenshot: () => provider });
  fixture.instance.listId = 'A';
  const importing = fixture.instance.onFile(imageEvent(file.promise));
  fixture.instance.listId = 'B';
  fixture.instance.typed('pinyin', input('B manual'));
  file.resolve('data:image/png;base64,aW1hZ2U=');
  await Promise.resolve();
  await Promise.resolve();
  provider.next(screenshot());
  await importing;
  assert.equal(fixture.instance.state().pinyin(), 'B manual');
  assert.deepEqual(fixture.instance.state().drafts(), []);
  fixture.instance.listId = 'A';
  assert.equal(fixture.instance.destination(), 'A');
  assert.equal(fixture.instance.state().drafts()[0].pinyin, 'zì');
});

test('provider failure retains manual fields and previously extracted screenshot rows', async () => {
  browserClock();
  const message = 'Provider key is not configured';
  let extractions = 0;
  const failure = () =>
    throwError(() => new HttpErrorResponse({ status: 503, error: { message } }));
  const fixture = component(WordImport, {
    drafts: failure,
    screenshot: () => (++extractions === 1 ? of(screenshot()) : failure()),
  });
  fixture.instance.typed('simplified', input('字'));
  fixture.instance.typed('pinyin', input('manual'));
  fixture.instance.fill();
  assert.equal(fixture.instance.state().pinyin(), 'manual');
  await fixture.instance.onFile(imageEvent());
  const row = fixture.instance.state().drafts()[0];
  await fixture.instance.onFile(imageEvent());
  assert.equal(fixture.instance.state().drafts()[0], row);
});

test('pending list creation prevents edits and duplicate creates; rejection retains the title', () => {
  browserClock();
  const pending = new Subject<unknown>();
  const submitted: string[] = [];
  const fixture = component(QueuePage, {
    createList: (title: string) => {
      submitted.push(title);
      return pending;
    },
  });
  fixture.instance.typed(input('First title'));
  fixture.instance.create();
  fixture.instance.typed(input('Replacement title'));
  fixture.instance.create();
  assert.deepEqual(submitted, ['First title']);
  assert.equal(fixture.instance.title(), 'First title');
  pending.error(new Error('transport failure'));
  assert.equal(fixture.instance.creating(), false);
  assert.equal(fixture.instance.title(), 'First title');
});

test('late import completion reloads only its destination and cannot leak busy state into another route', async () => {
  browserClock();
  const pending = new Subject<AddWordsResult>();
  const parentFixture = component(ListPage);
  const parent = parentFixture.instance;
  const child = component(WordImport, {
    screenshot: () => of(screenshot()),
    addWords: () => pending,
  }).instance;
  const busy = child.busy.subscribe((event) => parent.importBusy(event));
  const commits: string[] = [];
  const committed = child.committed.subscribe((id) => {
    commits.push(id);
    parent.importCommitted(id);
  });
  cleanups.push(() => {
    busy.unsubscribe();
    committed.unsubscribe();
  });
  await child.onFile(imageEvent());
  const saving = child.saveDrafts();
  assert.equal(parent.saving(), true);
  child.listId = 'B';
  parentFixture.route.next(convertToParamMap({ id: 'B' }));
  assert.equal(parent.saving(), false);
  pending.next({ added: 1, rejected: [] });
  await saving;
  assert.deepEqual(commits, ['list']);
  assert.equal(parent.saving(), false);
  child.listId = 'list';
  assert.deepEqual(child.state().drafts(), []);
});

test('example generation is single-flight and stale completion cannot replace a different word', () => {
  browserClock();
  const pending = new Subject<WordDetailData>();
  const ids: string[] = [];
  const fixture = component(WordExamples, {
    generateExample(id: string) {
      ids.push(id);
      return pending;
    },
  });
  fixture.instance.word = word;
  const changed: WordDetailData[] = [];
  const subscription = fixture.instance.changed.subscribe((value) => changed.push(value));
  cleanups.push(() => subscription.unsubscribe());
  fixture.instance.generate();
  fixture.instance.generate();
  fixture.instance.word = { ...word, id: 'different' };
  pending.next({ ...word, cards: [], examples: [{ id: 'sentence', ...example }] });
  assert.deepEqual(ids, ['word']);
  assert.deepEqual(changed, []);
  assert.equal(fixture.instance.generating().has(word.id), false);
});

test('a retried session cannot apply a retained resource value while its new launch is loading', () => {
  browserClock();
  let requested!: () => SessionRequest | undefined;
  const response = signal<Session | undefined>(undefined);
  const session = resource(() => (requested() ? response() : undefined));
  const fixture = component(StudyPage, {
    session(request: () => SessionRequest | undefined) {
      requested = request;
      return session;
    },
  });
  fixture.instance.start();
  session.error.set(new Error('offline'));
  fixture.flushEffects();
  assert.equal(fixture.instance.phase(), 'launch');
  assert.equal(fixture.instance.launching(), false);
  session.error.set(undefined);
  session.isLoading.set(true);
  response.set({ cards: [face] });
  fixture.instance.direction.set('production');
  fixture.instance.start();
  fixture.flushEffects();
  assert.equal(fixture.instance.launching(), true);
  assert.equal(fixture.instance.phase(), 'launch');
  assert.deepEqual(fixture.instance.cards(), []);
  response.set({ cards: [{ ...face, wordId: 'fresh', direction: 'production' }] });
  session.isLoading.set(false);
  fixture.flushEffects();
  assert.equal(fixture.instance.cards()[0].wordId, 'fresh');
  assert.equal(requested()?.direction, 'production');
});

test('overlapping imports to the same destination keep removal blocked until every writer completes', () => {
  browserClock();
  const fixture = component(ListPage);
  fixture.instance.importBusy({ listId: list.id, busy: true });
  fixture.instance.importBusy({ listId: list.id, busy: true });
  fixture.instance.importBusy({ listId: list.id, busy: false });
  assert.equal(fixture.instance.saving(), true);
  fixture.instance.importBusy({ listId: list.id, busy: false });
  assert.equal(fixture.instance.saving(), false);
});

test('screenshots opened from a system list can save into a selected personal list', async () => {
  browserClock();
  const writes: string[] = [];
  const fixture = component(WordImport, {
    screenshot: () => of(screenshot()),
    addWords: (id: string) => {
      writes.push(id);
      return of({ added: 1, rejected: [] });
    },
  });
  fixture.instance.listId = 'hsk-1';
  fixture.instance.editable = false;
  await fixture.instance.onFile(imageEvent());
  assert.equal(fixture.instance.destination(), 'list');
  fixture.instance.state().destination.set('B');
  await fixture.instance.saveDrafts();
  assert.deepEqual(writes, ['B']);
});
