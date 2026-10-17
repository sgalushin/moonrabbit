import '@angular/compiler';
import {
  createEnvironmentInjector,
  EnvironmentInjector,
  Injector,
  runInInjectionContext,
} from '@angular/core';
import { HttpErrorResponse } from '@angular/common/http';
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { Subject } from 'rxjs';
import { GradeRequest, GradeResult } from '@moonrabbit/contract';
import { Api } from './api';
import { Grades } from './grades';

const KEY = 'moonrabbit.grades';

function fixture() {
  const originals = ['window', 'navigator', 'localStorage'].map(
    (name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const,
  );
  const data = new Map<string, string>();
  const requests: { grade: GradeRequest; response: Subject<GradeResult> }[] = [];
  const locks = new Map<string, Promise<unknown>>();
  let quota = false;
  let beforeFlushRelease: (() => Promise<void>) | undefined;
  const storage = {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      if (quota) throw new Error('Storage quota exceeded');
      data.set(key, value);
    },
  };
  const lockManager = {
    async request(name: string, options: unknown, callback?: (lock: object | null) => unknown) {
      const work = typeof options === 'function' ? options : callback;
      assert.ok(work);
      if (
        options &&
        typeof options === 'object' &&
        'ifAvailable' in options &&
        options.ifAvailable === true &&
        locks.has(name)
      )
        return work(null);
      const previous = locks.get(name) || Promise.resolve();
      const current = previous.then(() => work({ name }));
      locks.set(name, current);
      try {
        return await current;
      } finally {
        if (name === `${KEY}.flush` && beforeFlushRelease) await beforeFlushRelease();
        if (locks.get(name) === current) locks.delete(name);
      }
    },
  };
  const events = new EventTarget();
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      addEventListener: events.addEventListener.bind(events),
      removeEventListener: events.removeEventListener.bind(events),
      setTimeout,
      clearTimeout,
    },
  });
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: { locks: lockManager },
  });
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: storage });
  const injector = createEnvironmentInjector(
    [
      {
        provide: Api,
        useValue: {
          grade(grade: GradeRequest) {
            const response = new Subject<GradeResult>();
            requests.push({ grade, response });
            return response;
          },
        },
      },
    ],
    Injector.NULL as EnvironmentInjector,
  );
  const grades = runInInjectionContext(injector, () => new Grades());
  return {
    grades,
    requests,
    data,
    setQuota(value: boolean) {
      quota = value;
    },
    onFlushRelease(hook: () => Promise<void>) {
      beforeFlushRelease = hook;
    },
    pending() {
      return JSON.parse(data.get(KEY) || '[]') as GradeRequest[];
    },
    dispose() {
      injector.destroy();
      for (const [name, descriptor] of originals) {
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else Reflect.deleteProperty(globalThis, name);
      }
    },
  };
}

function answer(eventId: string, reviewedAt: number): GradeRequest {
  return {
    eventId,
    wordId: 'w85103bfe9862e3f75083',
    direction: 'recognition',
    rating: 'good',
    reviewedAt,
  };
}

async function until(check: () => boolean) {
  for (let i = 0; i < 100; i++) {
    if (check()) return;
    await nextTurn();
  }
  throw new Error('Expected asynchronous state was not reached');
}

test('overlapping flushes retain a newer failed answer and submit each pending event in order', async () => {
  const f = fixture();
  try {
    await f.grades.enqueue(answer('first', 1000));
    await until(() => f.requests.length === 1);
    const worker = f.grades.flush();
    await f.grades.enqueue(answer('second', 2000));
    assert.equal(f.requests.length, 1, 'an enqueue must not start another HTTP worker');
    f.requests[0].response.next({ status: 'applied' });
    f.requests[0].response.complete();
    await until(() => f.requests.length === 2);
    f.requests[1].response.error(new HttpErrorResponse({ status: 0 }));
    await worker;
    assert.deepEqual(
      f.requests.map((request) => request.grade.eventId),
      ['first', 'second'],
    );
    assert.deepEqual(
      f.pending().map((grade) => grade.eventId),
      ['second'],
    );
    assert.equal(f.grades.pending(), 1);
  } finally {
    f.dispose();
  }
});

test('a failed older answer blocks newer answers until its acknowledgement', async () => {
  const f = fixture();
  try {
    await f.grades.enqueue(answer('older', 1000));
    await until(() => f.requests.length === 1);
    const firstFlush = f.grades.flush();
    await f.grades.enqueue(answer('newer', 2000));
    f.requests[0].response.error(new HttpErrorResponse({ status: 0 }));
    await firstFlush;
    assert.equal(f.requests.length, 1);
    assert.deepEqual(
      f.pending().map((grade) => grade.eventId),
      ['older', 'newer'],
    );
    const retry = f.grades.flush();
    await until(() => f.requests.length === 2);
    assert.equal(f.requests[1].grade.eventId, 'older');
    f.requests[1].response.next({ status: 'duplicate' });
    f.requests[1].response.complete();
    await until(() => f.requests.length === 3);
    assert.equal(f.requests[2].grade.eventId, 'newer');
    f.requests[2].response.next({ status: 'applied' });
    f.requests[2].response.complete();
    await retry;
    assert.deepEqual(f.pending(), []);
  } finally {
    f.dispose();
  }
});

test('stale acknowledgements preserve the pending answer for recovery', async () => {
  const f = fixture();
  try {
    await f.grades.enqueue(answer('conflict', 1000));
    await until(() => f.requests.length === 1);
    const worker = f.grades.flush();
    f.requests[0].response.next({ status: 'stale' });
    f.requests[0].response.complete();
    await worker;
    assert.deepEqual(
      f.pending().map((grade) => grade.eventId),
      ['conflict'],
    );
    assert.equal(f.grades.failed(), true);
  } finally {
    f.dispose();
  }
});

test('storage failures reject enqueue without claiming durable acceptance or overwriting corrupt data', async () => {
  const f = fixture();
  try {
    f.setQuota(true);
    await assert.rejects(f.grades.enqueue(answer('quota', 1000)), /quota/);
    assert.deepEqual(f.pending(), []);
    assert.equal(f.requests.length, 0);
    f.setQuota(false);
    f.data.set(KEY, '{broken');
    await assert.rejects(f.grades.enqueue(answer('corrupt', 2000)));
    assert.equal(f.data.get(KEY), '{broken');
    assert.equal(f.requests.length, 0);
  } finally {
    f.dispose();
  }
});

test('an answer enqueued after the worker reads an empty queue is not stranded', async () => {
  const f = fixture();
  try {
    let inserted = false;
    f.onFlushRelease(async () => {
      if (inserted) return;
      inserted = true;
      await f.grades.enqueue(answer('late-wakeup', 2000));
    });
    const worker = f.grades.flush();
    await until(() => f.requests.length === 1);
    f.requests[0].response.next({ status: 'applied' });
    f.requests[0].response.complete();
    await worker;
    assert.deepEqual(f.pending(), []);
  } finally {
    f.dispose();
  }
});
