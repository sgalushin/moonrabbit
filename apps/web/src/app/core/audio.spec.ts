import '@angular/compiler';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Injector, runInInjectionContext } from '@angular/core';
import { Subject } from 'rxjs';
import type { Face } from '@moonrabbit/contract';
import { Api } from './api';
import { AudioCache } from './audio';

const face = (wordId: string): Face => ({
  wordId,
  direction: 'recognition',
  simplified: '长',
  pinyin: 'cháng',
  translation: 'long',
});
const clip = () => new Uint8Array([0x49, 0x44, 0x33, 0x04]).buffer;

function fixture() {
  const requests = new Map<string, Subject<ArrayBuffer>>();
  const items = new Map<string, string>();
  const created: string[] = [];
  const revoked: string[] = [];
  let quota = false;
  let playFailure = false;
  let decodeFailure = false;
  class FakeAudio {
    static instances: FakeAudio[] = [];
    paused = false;
    cleared = false;
    loaded = false;
    onended: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(public src: string) {
      FakeAudio.instances.push(this);
    }
    pause() {
      this.paused = true;
    }
    removeAttribute(name: string) {
      if (name === 'src') this.cleared = true;
    }
    load() {
      this.loaded = true;
    }
    play() {
      if (decodeFailure) {
        decodeFailure = false;
        this.onerror?.();
        return Promise.reject(new Error('Invalid cached MP3'));
      }
      return playFailure ? Promise.reject(new Error('Playback failed')) : Promise.resolve();
    }
  }
  const originalAudio = Object.getOwnPropertyDescriptor(globalThis, 'Audio');
  const originalStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const originalCreate = URL.createObjectURL;
  const originalRevoke = URL.revokeObjectURL;
  Object.defineProperty(globalThis, 'Audio', { configurable: true, value: FakeAudio });
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => items.get(key) ?? null,
      removeItem: (key: string) => items.delete(key),
      setItem: (key: string, value: string) => {
        if (quota) throw new Error('Quota exceeded');
        items.set(key, value);
      },
    },
  });
  URL.createObjectURL = () => {
    const url = `blob:test-${created.length}`;
    created.push(url);
    return url;
  };
  URL.revokeObjectURL = (url) => {
    revoked.push(url);
  };
  const injector = Injector.create({
    providers: [
      {
        provide: Api,
        useValue: {
          speech: (wordId: string, sentenceId?: string) => {
            const request = new Subject<ArrayBuffer>();
            requests.set(sentenceId ? `${wordId}/${sentenceId}` : wordId, request);
            return request;
          },
        },
      },
    ],
  });
  const cache = runInInjectionContext(injector, () => new AudioCache());
  return {
    cache,
    requests,
    items,
    created,
    revoked,
    FakeAudio,
    respond(wordId: string) {
      const request = requests.get(wordId);
      assert.ok(request);
      request.next(clip());
    },
    quota: () => {
      quota = true;
    },
    failPlayback: () => {
      playFailure = true;
    },
    failCachedDecode: () => {
      decodeFailure = true;
    },
    restore: () => {
      cache.stop();
      if (originalAudio) Object.defineProperty(globalThis, 'Audio', originalAudio);
      else Reflect.deleteProperty(globalThis, 'Audio');
      if (originalStorage) Object.defineProperty(globalThis, 'localStorage', originalStorage);
      else Reflect.deleteProperty(globalThis, 'localStorage');
      URL.createObjectURL = originalCreate;
      URL.revokeObjectURL = originalRevoke;
      injector.destroy();
    },
  };
}

test('a late older response never interrupts newer audio', async () => {
  const f = fixture();
  try {
    const older = f.cache.play(face('older'));
    const newer = f.cache.play(face('newer'));
    f.respond('newer');
    await newer;
    const playing = f.FakeAudio.instances[0];
    f.respond('older');
    await older;
    assert.equal(f.FakeAudio.instances.length, 1);
    assert.equal(playing.paused, false);
    assert.equal(f.created.length, 1);
    assert.deepEqual(f.revoked, []);
  } finally {
    f.restore();
  }
});

test('replacement and disposal revoke URLs and invalidate pending retrieval', async () => {
  const f = fixture();
  try {
    const initial = f.cache.play(face('initial'));
    f.respond('initial');
    await initial;
    const first = f.FakeAudio.instances[0];
    const replacement = f.cache.play(face('replacement'));
    assert.equal(first.paused, true);
    assert.equal(first.cleared, true);
    assert.equal(first.loaded, true);
    assert.deepEqual(f.revoked, [f.created[0]]);
    f.cache.ngOnDestroy();
    f.respond('replacement');
    await replacement;
    assert.equal(f.created.length, 1);
    assert.equal(f.FakeAudio.instances.length, 1);
  } finally {
    f.restore();
  }
});

test('playback rejection releases its URL', async () => {
  const f = fixture();
  try {
    f.failPlayback();
    const playback = f.cache.play(face('rejected'));
    f.respond('rejected');
    await assert.rejects(playback);
    assert.deepEqual(f.revoked, f.created);
    assert.equal(f.FakeAudio.instances[0].paused, true);
  } finally {
    f.restore();
  }
});

test('corrupt cached bytes and quota failures still allow retrieved audio to play', async () => {
  const f = fixture();
  try {
    f.items.set('moonrabbit.audio.v2.word|cháng', 'not base64!');
    f.items.set('moonrabbit.audio.index.v2', '[]');
    f.quota();
    const playback = f.cache.play(face('word'));
    f.respond('word');
    await playback;
    assert.equal(f.FakeAudio.instances.length, 1);
    assert.equal(f.FakeAudio.instances[0].paused, false);
    assert.equal(f.items.has('moonrabbit.audio.v2.word|cháng'), false);
    const ended = f.FakeAudio.instances[0].onended;
    assert.ok(ended);
    ended();
    assert.deepEqual(f.revoked, f.created);
  } finally {
    f.restore();
  }
});

test('a cached media decoding error discards the clip and fetches playable audio', async () => {
  const f = fixture();
  try {
    f.items.set(
      'moonrabbit.audio.v2.word|cháng',
      btoa(String.fromCharCode(...new Uint8Array(clip()))),
    );
    f.failCachedDecode();
    const playback = f.cache.play(face('word'));
    await Promise.resolve();
    assert.deepEqual(f.revoked, [f.created[0]]);
    f.respond('word');
    await playback;
    assert.equal(f.FakeAudio.instances.length, 2);
    assert.equal(f.FakeAudio.instances[1].paused, false);
    assert.equal(f.created.length, 2);
  } finally {
    f.restore();
  }
});

test('a media error releases the current URL', async () => {
  const f = fixture();
  try {
    const playback = f.cache.play(face('word'));
    f.respond('word');
    await playback;
    const errored = f.FakeAudio.instances[0].onerror;
    assert.ok(errored);
    errored();
    assert.deepEqual(f.revoked, f.created);
    assert.equal(f.FakeAudio.instances[0].paused, true);
    assert.equal(f.items.has('moonrabbit.audio.v2.word|cháng'), false);
  } finally {
    f.restore();
  }
});

test('sentence clips use authoritative sentence IDs and never collide with word clips', async () => {
  const f = fixture();
  try {
    const wordAudio = f.cache.play(face('word'));
    f.respond('word');
    await wordAudio;
    const sentenceAudio = f.cache.play(face('word'), 'sentence-one');
    assert.ok(f.requests.has('word/sentence-one'));
    f.respond('word/sentence-one');
    await sentenceAudio;
    assert.equal(f.FakeAudio.instances.length, 2);
    assert.ok(f.items.has('moonrabbit.audio.v2.word|sentence:sentence-one|cháng'));
    assert.ok(f.items.has('moonrabbit.audio.v2.word|cháng'));
    const cachedSentence = f.cache.play(face('word'), 'sentence-one');
    await cachedSentence;
    assert.equal(f.FakeAudio.instances.length, 3);
    const otherSentence = f.cache.play(face('word'), 'sentence-two');
    f.respond('word/sentence-two');
    await otherSentence;
    assert.ok(f.items.has('moonrabbit.audio.v2.word|sentence:sentence-two|cháng'));
  } finally {
    f.restore();
  }
});
