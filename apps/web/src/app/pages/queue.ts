import { Component, DestroyRef, computed, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { RouterLink } from '@angular/router';
import { LucideChevronDown, LucideChevronUp, LucidePlus, LucideX } from '@lucide/angular';
import { Api } from '../core/api';
import { Shell } from '../shell';

@Component({
  selector: 'app-queue',
  imports: [Shell, RouterLink, LucideChevronUp, LucideChevronDown, LucideX, LucidePlus],
  templateUrl: './queue.html',
})
export class QueuePage {
  private api = inject(Api);
  private destroyRef = inject(DestroyRef);
  readonly dashboard = this.api.dashboard();
  readonly listResource = this.api.lists();
  readonly lists = computed(() => (this.listResource.hasValue() ? this.listResource.value() : []));
  readonly queue = computed(() => (this.dashboard.hasValue() ? this.dashboard.value().queue : []));
  title = signal('');
  error = signal('');
  readonly loading = computed(() => this.dashboard.isLoading() || this.listResource.isLoading());
  saving = signal(false);
  creating = signal(false);
  readonly loadFailed = computed(() => !!this.dashboard.error() || !!this.listResource.error());
  readonly loadError = computed(() =>
    this.loadFailed()
      ? 'The queue and lists could not be loaded. Retry loading before editing.'
      : '',
  );
  private dragIndex = -1;

  reload(preserveError = false) {
    if (!preserveError) this.error.set('');
    this.dashboard.reload();
    this.listResource.reload();
  }

  busy() {
    return (
      this.loading() ||
      this.saving() ||
      this.creating() ||
      this.loadFailed() ||
      !this.dashboard.hasValue() ||
      !this.listResource.hasValue()
    );
  }

  available() {
    const queued = new Set(this.queue().map((list) => list.id));
    return this.lists().filter((list) => !queued.has(list.id));
  }

  save(ids: string[]) {
    if (this.busy() || !this.dashboard.hasValue()) return;
    this.saving.set(true);
    this.error.set('');
    this.api
      .setQueue(ids, this.dashboard.value().queueVersion)
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: () => {
          this.saving.set(false);
          this.reload();
        },
        error: (error) => {
          this.saving.set(false);
          this.error.set(
            error.status === 409
              ? 'The queue changed elsewhere. Its current order has been reloaded; try your edit again.'
              : 'The queue save could not be confirmed. Reloading the server order before another edit.',
          );
          this.reload(true);
        },
      });
  }

  add(id: string) {
    this.save(
      this.queue()
        .map((list) => list.id)
        .concat(id),
    );
  }

  remove(id: string) {
    this.save(
      this.queue()
        .map((list) => list.id)
        .filter((item) => item !== id),
    );
  }

  move(index: number, delta: number) {
    const ids = this.queue().map((list) => list.id);
    const next = index + delta;
    if (next < 0 || next >= ids.length) return;
    const [item] = ids.splice(index, 1);
    if (item) ids.splice(next, 0, item);
    this.save(ids);
  }

  drag(index: number) {
    if (this.busy()) return;
    this.dragIndex = index;
  }

  drop(index: number) {
    if (this.busy() || this.dragIndex < 0 || this.dragIndex === index) return;
    const ids = this.queue().map((list) => list.id);
    const [item] = ids.splice(this.dragIndex, 1);
    if (item) ids.splice(index, 0, item);
    this.dragIndex = -1;
    this.save(ids);
  }

  create() {
    const title = this.title().trim();
    if (!title || this.busy()) return;
    this.creating.set(true);
    this.error.set('');
    this.api
      .createList(title)
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: (list) => {
          this.creating.set(false);
          this.title.set('');
          this.add(list.id);
        },
        error: () => {
          this.creating.set(false);
          this.error.set('The list could not be created. Your title is still here; try again.');
        },
      });
  }

  typed(event: Event) {
    if (this.busy()) return;
    this.title.set((event.target as HTMLInputElement).value);
  }
}
