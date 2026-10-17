import { DestroyRef, Injectable, inject, signal } from '@angular/core';
import { HttpErrorResponse } from '@angular/common/http';
import { firstValueFrom } from 'rxjs';
import { GradeRequest } from '@moonrabbit/contract';
import { Api } from './api';
import { apiErrorMessage } from './errors';

const KEY = 'moonrabbit.grades';
const STORE_LOCK = `${KEY}.store`;
const FLUSH_LOCK = `${KEY}.flush`;

@Injectable({ providedIn: 'root' })
export class Grades {
  private api = inject(Api);
  private flushing: Promise<void> | null = null;
  private flushRequested = false;
  private retryTimer: number | undefined;
  pending = signal(0);
  failed = signal(false);
  error = signal('');

  constructor() {
    this.refresh();
    const changed = (event: StorageEvent) => {
      if (event.key === KEY) {
        this.refresh();
        void this.flush();
      }
    };
    const online = () => void this.flush();
    window.addEventListener('storage', changed);
    window.addEventListener('online', online);
    inject(DestroyRef).onDestroy(() => {
      window.removeEventListener('storage', changed);
      window.removeEventListener('online', online);
      window.clearTimeout(this.retryTimer);
    });
  }

  async enqueue(grade: GradeRequest): Promise<void> {
    try {
      await this.lockStore(() => {
        const queue = this.read();
        if (!queue.some((item) => item.eventId === grade.eventId)) queue.push(grade);
        queue.sort((a, b) => a.reviewedAt - b.reviewedAt || a.eventId.localeCompare(b.eventId));
        this.save(queue);
      });
    } catch (error) {
      this.fail(error);
      throw error;
    }
    void this.flush();
  }

  flush(): Promise<void> {
    this.flushRequested = true;
    if (this.flushing) return this.flushing;
    this.flushing = this.drain()
      .catch((error) => this.fail(error))
      .finally(() => {
        this.flushing = null;
        // A wakeup can arrive after the last empty read but before promise settlement.
        if (this.flushRequested && !this.failed()) void this.flush();
      });
    return this.flushing;
  }

  exportPending(): void {
    const content = localStorage.getItem(KEY) || '[]';
    const url = URL.createObjectURL(new Blob([content], { type: 'application/json' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = 'moonrabbit-pending-answers.json';
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  private async drain(): Promise<void> {
    do {
      this.flushRequested = false;
      if (!(await this.runFlush())) {
        this.flushRequested = false;
        return;
      }
    } while (this.flushRequested);
  }

  private async runFlush(): Promise<boolean> {
    this.requireLocks();
    const completed = await navigator.locks.request(FLUSH_LOCK, async () => {
      window.clearTimeout(this.retryTimer);
      while (true) {
        const grade = await this.lockStore(() => this.read()[0]);
        if (!grade) {
          this.failed.set(false);
          this.error.set('');
          this.pending.set(0);
          return true;
        }
        try {
          const result = await firstValueFrom(this.api.grade(grade));
          if (result.status === 'stale') {
            throw new Error(
              'A pending answer conflicts with newer saved history. It has been kept for recovery.',
            );
          }
          if (result.status !== 'applied' && result.status !== 'duplicate') {
            throw new Error('The server did not acknowledge this answer. It remains pending.');
          }
          await this.lockStore(() =>
            this.save(this.read().filter((item) => item.eventId !== grade.eventId)),
          );
        } catch (error) {
          this.fail(error);
          // Do not send newer answers ahead of an unacknowledged answer.
          if (error instanceof HttpErrorResponse && (error.status === 0 || error.status >= 500)) {
            this.retryTimer = window.setTimeout(() => void this.flush(), 1500);
          }
          return false;
        }
      }
    });
    this.refresh();
    return completed;
  }

  private lockStore<T>(work: () => T): Promise<T> {
    this.requireLocks();
    return navigator.locks.request(STORE_LOCK, work);
  }

  private requireLocks() {
    if (!navigator.locks)
      throw new Error('Saving answers requires a browser with Web Locks on a secure local origin.');
  }

  private save(queue: GradeRequest[]) {
    localStorage.setItem(KEY, JSON.stringify(queue));
    this.pending.set(queue.length);
  }

  private read(): GradeRequest[] {
    const parsed: unknown = JSON.parse(localStorage.getItem(KEY) || '[]');
    if (!Array.isArray(parsed) || !parsed.every(isGrade)) {
      throw new Error('Pending answer storage is invalid. It has not been overwritten.');
    }
    return parsed;
  }

  private refresh() {
    try {
      const count = this.read().length;
      this.pending.set(count);
      if (!count) {
        this.failed.set(false);
        this.error.set('');
      }
    } catch (error) {
      this.fail(error);
    }
  }

  private fail(error: unknown) {
    this.failed.set(true);
    this.error.set(apiErrorMessage(error, 'An answer has not been saved yet. It remains pending.'));
  }
}

function isGrade(value: unknown): value is GradeRequest {
  if (
    !value ||
    typeof value !== 'object' ||
    !('eventId' in value) ||
    !('wordId' in value) ||
    !('direction' in value) ||
    !('rating' in value) ||
    !('reviewedAt' in value)
  )
    return false;
  return (
    typeof value.eventId === 'string' &&
    value.eventId.length > 0 &&
    typeof value.wordId === 'string' &&
    value.wordId.length > 0 &&
    (value.direction === 'recognition' || value.direction === 'production') &&
    (value.rating === 'again' || value.rating === 'good') &&
    typeof value.reviewedAt === 'number' &&
    Number.isSafeInteger(value.reviewedAt) &&
    value.reviewedAt >= 0
  );
}
