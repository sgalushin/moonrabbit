import { Component, DestroyRef, ViewChild, computed, effect, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { ActivatedRoute } from '@angular/router';
import { LIST_PAGE_SIZE, Word } from '@moonrabbit/contract';
import { LucideLoaderCircle, LucideVolume2 } from '@lucide/angular';
import { AudioCache } from '../core/audio';
import { Api } from '../core/api';
import { apiErrorMessage } from '../core/errors';
import { Pinyin } from '../pinyin';
import { Shell } from '../shell';
import { WordImport } from './word-import';
import { WordDetail } from './word-detail';

@Component({
  selector: 'app-list',
  imports: [Shell, Pinyin, WordImport, WordDetail, LucideVolume2, LucideLoaderCircle],
  templateUrl: './list.html',
})
export class ListPage {
  private api = inject(Api);
  private route = inject(ActivatedRoute);
  readonly audio = inject(AudioCache);
  private destroyRef = inject(DestroyRef);
  @ViewChild(WordDetail) detail?: WordDetail;
  id = '';
  readonly pageSize = LIST_PAGE_SIZE;
  private request = signal<{ id: string; cursor: number } | undefined>(undefined);
  readonly listResource = this.api.list(() => this.request());
  private page = computed(() =>
    this.listResource.hasValue() ? this.listResource.value() : undefined,
  );
  readonly title = computed(() => this.page()?.title ?? '');
  readonly owner = computed(() => this.page()?.owner ?? '');
  readonly words = computed(() => this.page()?.words ?? []);
  readonly cursor = computed(() => this.request()?.cursor ?? 0);
  readonly nextCursor = computed(() => this.page()?.nextCursor ?? null);
  readonly totalWords = computed(() => this.page()?.totalWords ?? 0);
  readonly loading = this.listResource.isLoading;
  readonly loadError = computed(() =>
    this.listResource.error()
      ? apiErrorMessage(this.listResource.error(), 'This page of the list could not be loaded.')
      : '',
  );
  error = signal('');
  playingId = signal<string | null>(null);
  private editorBusy = signal(false);
  private removing = signal(false);
  saving = computed(() => this.editorBusy() || this.removing());
  private busyByList = new Map<string, number>();
  private removals = new Set<string>();
  private mutationErrors = new Map<string, string>();
  private destroyed = false;

  constructor() {
    this.destroyRef.onDestroy(() => {
      this.destroyed = true;
      this.audio.stop();
    });
    this.route.paramMap.pipe(takeUntilDestroyed(this.destroyRef)).subscribe((params) => {
      this.detail?.close();
      this.audio.stop();
      this.playingId.set(null);
      this.id = params.get('id') || '';
      this.editorBusy.set((this.busyByList.get(this.id) ?? 0) > 0);
      this.removing.set(this.removals.has(this.id));
      this.error.set(this.mutationErrors.get(this.id) ?? '');
      this.request.set(this.id ? { id: this.id, cursor: 0 } : undefined);
    });
    effect(() => {
      const request = this.request();
      if (!request || this.listResource.isLoading() || !this.listResource.hasValue()) return;
      const page = this.listResource.value();
      // Another editor may empty the final page while this route remains open.
      if (page.id === request.id && request.cursor > 0 && request.cursor >= page.totalWords) {
        this.request.set({
          id: request.id,
          cursor: Math.max(0, Math.floor((page.totalWords - 1) / LIST_PAGE_SIZE) * LIST_PAGE_SIZE),
        });
      }
    });
  }

  load(cursor = this.cursor()) {
    if (!this.id) return;
    if (cursor === this.cursor()) this.listResource.reload();
    else this.request.set({ id: this.id, cursor });
  }

  importBusy(event: { listId: string; busy: boolean }) {
    const count = Math.max(0, (this.busyByList.get(event.listId) ?? 0) + (event.busy ? 1 : -1));
    if (count) this.busyByList.set(event.listId, count);
    else this.busyByList.delete(event.listId);
    if (event.listId === this.id) this.editorBusy.set(count > 0);
  }

  importCommitted(listId: string) {
    if (listId === this.id) this.load();
  }

  remove(wordId: string) {
    if (this.saving() || this.loading() || !this.owner() || this.owner() === 'system') return;
    const id = this.id;
    const cursor = this.cursor();
    const previousPage = this.words().length === 1 && cursor > 0;
    this.removing.set(true);
    this.error.set('');
    this.removals.add(id);
    this.mutationErrors.delete(id);
    this.api
      .removeWord(id, wordId)
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: () => {
          this.removals.delete(id);
          if (id !== this.id) return;
          this.removing.set(false);
          this.load(
            this.cursor() === cursor && previousPage
              ? Math.max(0, cursor - LIST_PAGE_SIZE)
              : this.cursor(),
          );
        },
        error: (error: unknown) => {
          this.removals.delete(id);
          const message =
            apiErrorMessage(error, 'That word could not be removed.') + ' Try Remove again.';
          this.mutationErrors.set(id, message);
          if (id !== this.id) return;
          this.removing.set(false);
          this.error.set(message);
        },
      });
  }

  async play(word: Word) {
    const id = this.id;
    this.playingId.set(word.id);
    try {
      await this.audio.play({
        wordId: word.id,
        direction: 'recognition',
        simplified: word.simplified,
        pinyin: word.pinyin,
        translation: word.translation,
        pronouncedPinyin: word.pronouncedPinyin,
      });
    } catch (error: unknown) {
      if (!this.destroyed && id === this.id)
        this.error.set(
          apiErrorMessage(error, 'This word could not be played.') + ' Try Play again.',
        );
    } finally {
      if (id === this.id && this.playingId() === word.id) this.playingId.set(null);
    }
  }
}
