import {
  Component,
  DestroyRef,
  EventEmitter,
  Input,
  Output,
  computed,
  inject,
  signal,
} from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  ExampleSentenceInput,
  WORD_BATCH_SIZE,
  Word,
  WordInput,
  WordSearchResult,
} from '@moonrabbit/contract';
import { firstValueFrom } from 'rxjs';
import { Api } from '../core/api';
import { AudioCache } from '../core/audio';
import { apiErrorMessage } from '../core/errors';
import { Pinyin } from '../pinyin';

const MAX_WORD_REQUEST_BYTES = 1_000_000;

interface DraftRow extends WordInput {
  id: number;
  pronouncedPinyin: string;
  examples: ExampleSentenceInput[];
  matches: WordSearchResult[];
  errors: Record<string, string>;
}

class EditorState {
  constructor(readonly listId: string) {}
  query = signal('');
  searchQuery = signal('');
  simplified = signal('');
  pinyin = signal('');
  pronouncedPinyin = signal('');
  translation = signal('');
  filling = signal(false);
  saving = signal(false);
  extracting = signal(false);
  destination = signal('');
  drafts = signal<DraftRow[]>([]);
  warnings = signal<string[]>([]);
  draftCursor = signal(0);
  visibleDrafts = computed(() =>
    this.drafts().slice(this.draftCursor(), this.draftCursor() + WORD_BATCH_SIZE),
  );
  error = signal('');
  saveErrors = signal<Record<string, string>>({});
  manualRevision = 0;
  searchTimer = 0;
}

@Component({
  selector: 'app-word-import',
  imports: [Pinyin],
  templateUrl: './word-import.html',
})
export class WordImport {
  private api = inject(Api);
  private audio = inject(AudioCache);
  private destroyRef = inject(DestroyRef);
  private editors = new Map<string, EditorState>();
  private draftId = 0;
  private destroyed = false;
  private playback = 0;
  readonly batchSize = WORD_BATCH_SIZE;
  readonly state = signal(new EditorState(''));
  readonly listResource = this.api.lists();
  readonly personalLists = computed(() =>
    this.listResource.hasValue()
      ? this.listResource.value().filter((list) => list.owner !== 'system')
      : [],
  );
  readonly destination = computed(() => {
    const state = this.state();
    return (
      state.destination() ||
      this.personalLists().find((list) => list.id === state.listId)?.id ||
      this.personalLists()[0]?.id ||
      ''
    );
  });
  readonly destinationAvailable = computed(() =>
    this.personalLists().some((list) => list.id === this.destination()),
  );
  readonly searchResource = this.api.search(() => this.state().searchQuery().trim() || undefined);
  readonly results = computed(() =>
    this.searchResource.hasValue() ? this.searchResource.value() : [],
  );
  readonly searchError = computed(() =>
    this.searchResource.error()
      ? apiErrorMessage(this.searchResource.error(), 'Search could not be completed.')
      : '',
  );
  readonly playing = signal<string | null>(null);

  @Input({ required: true }) set listId(id: string) {
    let state = this.editors.get(id);
    if (!state) {
      state = new EditorState(id);
      this.editors.set(id, state);
    }
    this.state.set(state);
    this.audio.stop();
    this.playback++;
    this.playing.set(null);
  }
  @Input() editable = false;
  @Input() disabled = false;
  @Output() busy = new EventEmitter<{ listId: string; busy: boolean }>();
  @Output() committed = new EventEmitter<string>();

  constructor() {
    this.destroyRef.onDestroy(() => {
      this.destroyed = true;
      this.playback++;
      this.audio.stop();
      for (const state of this.editors.values()) window.clearTimeout(state.searchTimer);
    });
  }

  search(event: Event) {
    const state = this.state();
    const value = (event.target as HTMLInputElement).value;
    state.query.set(value);
    state.searchQuery.set('');
    window.clearTimeout(state.searchTimer);
    if (!value.trim()) return;
    state.searchTimer = window.setTimeout(() => state.searchQuery.set(value), 200);
  }

  addExisting(word: Word) {
    const state = this.state();
    if (this.disabled || !this.editable || state.saving()) return;
    state.saving.set(true);
    this.busy.emit({ listId: state.listId, busy: true });
    state.error.set('');
    const { simplified, pinyin, pronouncedPinyin, translation } = word;
    this.api
      .addWords(state.listId, [{ simplified, pinyin, pronouncedPinyin, translation }])
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: (result) => {
          state.saving.set(false);
          this.busy.emit({ listId: state.listId, busy: false });
          if (result.rejected.length) {
            state.error.set(result.rejected[0].error);
            return;
          }
          // A later query belongs to a different intent and must not be cleared.
          this.committed.emit(state.listId);
          if (this.state() === state) this.searchResource.reload();
        },
        error: (error: unknown) => {
          state.saving.set(false);
          this.busy.emit({ listId: state.listId, busy: false });
          state.error.set(
            apiErrorMessage(error, 'That word could not be added.') + ' Try Add again.',
          );
        },
      });
  }

  fill() {
    const state = this.state();
    const text = state.simplified().trim();
    if (!text || this.disabled || !this.editable || state.filling() || state.saving()) return;
    const revision = state.manualRevision;
    state.filling.set(true);
    state.error.set('');
    this.api
      .drafts([text])
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: (rows) => {
          state.filling.set(false);
          if (revision !== state.manualRevision) return;
          const reading = rows[0]?.readings[0];
          if (!reading) {
            state.error.set('No reading was returned. Type the fields yourself or try Fill again.');
            return;
          }
          state.pinyin.set(reading.pinyin);
          state.pronouncedPinyin.set(reading.pronouncedPinyin ?? '');
          state.translation.set(reading.translation);
        },
        error: (error: unknown) => {
          state.filling.set(false);
          if (revision !== state.manualRevision) return;
          state.error.set(
            apiErrorMessage(error, 'The fill could not be completed.') +
              ' You can type the fields yourself or try Fill again.',
          );
        },
      });
  }

  saveManual() {
    const state = this.state();
    if (this.disabled || !this.editable || state.saving()) return;
    const revision = state.manualRevision;
    state.saving.set(true);
    this.busy.emit({ listId: state.listId, busy: true });
    state.error.set('');
    this.api
      .addWords(state.listId, [
        {
          simplified: state.simplified(),
          pinyin: state.pinyin(),
          pronouncedPinyin: state.pronouncedPinyin(),
          translation: state.translation(),
        },
      ])
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: (result) => {
          state.saving.set(false);
          this.busy.emit({ listId: state.listId, busy: false });
          if (result.rejected.length) {
            state.error.set(result.rejected[0].error);
            return;
          }
          if (revision === state.manualRevision) {
            state.manualRevision++;
            state.simplified.set('');
            state.pinyin.set('');
            state.pronouncedPinyin.set('');
            state.translation.set('');
          }
          this.committed.emit(state.listId);
        },
        error: (error: unknown) => {
          state.saving.set(false);
          this.busy.emit({ listId: state.listId, busy: false });
          state.error.set(
            apiErrorMessage(error, 'That word could not be saved.') +
              ' Your fields are retained; try Save again.',
          );
        },
      });
  }

  async onFile(event: Event) {
    const state = this.state();
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file || this.disabled || state.saving() || state.extracting()) return;
    state.error.set('');
    if (
      !['image/png', 'image/jpeg', 'image/webp'].includes(file.type) ||
      file.size > 8 * 1024 * 1024
    ) {
      state.error.set('Choose a PNG, JPEG, or WebP screenshot no larger than 8 MiB.');
      return;
    }
    state.extracting.set(true);
    if (!state.destination()) state.destination.set(this.destination());
    try {
      const image = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        const release = this.destroyRef.onDestroy(() => {
          if (reader.readyState === FileReader.LOADING) reader.abort();
        });
        reader.onload = () => {
          release();
          if (typeof reader.result === 'string') resolve(reader.result);
          else reject(new Error('The image could not be read.'));
        };
        reader.onerror = () => {
          release();
          reject(reader.error ?? new Error('The image could not be read.'));
        };
        reader.onabort = () => {
          release();
          reject(new Error('Image reading was cancelled.'));
        };
        reader.readAsDataURL(file);
      });
      if (this.destroyed) return;
      const result = await firstValueFrom(
        this.api.screenshot(image).pipe(takeUntilDestroyed(this.destroyRef)),
      );
      if (this.destroyed) return;
      const rows = result.words.flatMap((word): DraftRow[] =>
        word.readings.map((reading) => ({
          id: ++this.draftId,
          simplified: word.simplified,
          pinyin: reading.pinyin,
          pronouncedPinyin: reading.pronouncedPinyin ?? '',
          translation: reading.translation,
          examples: (reading.examples ?? []).map((example) => ({ ...example })),
          matches: word.matches,
          errors: {},
        })),
      );
      state.drafts.update((current) => [...current, ...rows]);
      state.warnings.update((current) => [...current, ...result.warnings]);
      if (!rows.length)
        state.error.set('No vocabulary was found in this screenshot. Nothing was saved.');
    } catch (error: unknown) {
      if (!this.destroyed)
        state.error.set(
          apiErrorMessage(error, 'Screenshot extraction failed.') +
            ' Nothing from this screenshot was saved. Select it again to retry; earlier preview rows are retained.',
        );
    } finally {
      state.extracting.set(false);
    }
  }

  setDraft(
    row: DraftRow,
    field: 'simplified' | 'pinyin' | 'pronouncedPinyin' | 'translation',
    event: Event,
  ) {
    const state = this.state();
    if (this.disabled || state.saving()) return;
    row[field] = (event.target as HTMLInputElement).value;
    delete row.errors[this.destination()];
    state.drafts.set(state.drafts().slice());
  }

  setExample(
    row: DraftRow,
    example: ExampleSentenceInput,
    field: keyof ExampleSentenceInput,
    event: Event,
  ) {
    const state = this.state();
    if (this.disabled || state.saving()) return;
    example[field] = (event.target as HTMLInputElement).value;
    delete row.errors[this.destination()];
    state.drafts.set(state.drafts().slice());
  }

  async saveDrafts() {
    const state = this.state();
    const destination = this.destination();
    if (
      this.disabled ||
      state.saving() ||
      state.extracting() ||
      !state.drafts().length ||
      !this.destinationAvailable()
    )
      return;
    const rows = state.drafts().slice();
    const inputs = rows.map(({ simplified, pinyin, pronouncedPinyin, translation, examples }) => ({
      simplified,
      pinyin,
      pronouncedPinyin,
      translation,
      examples,
    }));
    // Capture the destination before any async work. Route changes must not retarget writes or errors.
    state.destination.set(destination);
    state.saving.set(true);
    this.busy.emit({ listId: destination, busy: true });
    state.saveErrors.update((errors) => ({ ...errors, [destination]: '' }));
    try {
      const encoder = new TextEncoder();
      for (let offset = 0; offset < rows.length && !this.destroyed;) {
        let end = Math.min(offset + WORD_BATCH_SIZE, rows.length);
        // Source sentences can hit the API's 1 MiB limit before the 40-word limit.
        while (
          end > offset + 1 &&
          encoder.encode(JSON.stringify({ words: inputs.slice(offset, end) })).byteLength >
            MAX_WORD_REQUEST_BYTES
        ) {
          end = offset + Math.floor((end - offset) / 2);
        }
        const chunk = rows.slice(offset, end);
        const result = await firstValueFrom(
          this.api
            .addWords(destination, inputs.slice(offset, end))
            .pipe(takeUntilDestroyed(this.destroyRef)),
        );
        offset = end;
        if (this.destroyed) return;
        const rejected = new Map(result.rejected.map((row) => [row.index, row.error]));
        const accepted = new Set(
          chunk.filter((row, index) => {
            row.errors[destination] = rejected.get(index) || '';
            return !rejected.has(index);
          }),
        );
        state.drafts.update((current) => current.filter((row) => !accepted.has(row)));
        state.draftCursor.set(
          Math.min(
            state.draftCursor(),
            Math.max(
              0,
              Math.floor((state.drafts().length - 1) / WORD_BATCH_SIZE) * WORD_BATCH_SIZE,
            ),
          ),
        );
        if (accepted.size) this.committed.emit(destination);
      }
      if (state.drafts().length)
        state.saveErrors.update((errors) => ({
          ...errors,
          [destination]:
            'Some rows were rejected. They remain below with their errors; correct them and save again.',
        }));
      if (this.state() === state) this.searchResource.reload();
    } catch (error: unknown) {
      if (!this.destroyed)
        state.saveErrors.update((errors) => ({
          ...errors,
          [destination]:
            apiErrorMessage(error, 'Saving stopped.') +
            ' All unconfirmed rows remain below. Try Add these words again; confirmed rows were removed.',
        }));
    } finally {
      state.saving.set(false);
      if (!this.destroyed) this.busy.emit({ listId: destination, busy: false });
    }
  }

  typed(field: 'simplified' | 'pinyin' | 'pronouncedPinyin' | 'translation', event: Event) {
    const state = this.state();
    if (this.disabled || state.saving()) return;
    state.manualRevision++;
    state[field].set((event.target as HTMLInputElement).value);
  }

  async play(word: Word) {
    const state = this.state();
    const generation = ++this.playback;
    this.playing.set(word.id);
    try {
      await this.audio.play({
        wordId: word.id,
        direction: 'recognition',
        simplified: word.simplified,
        pinyin: word.pinyin,
        pronouncedPinyin: word.pronouncedPinyin,
        translation: word.translation,
      });
    } catch (error: unknown) {
      if (!this.destroyed && generation === this.playback)
        state.error.set(apiErrorMessage(error, 'Audio could not be played. Try again.'));
    } finally {
      if (generation === this.playback) this.playing.set(null);
    }
  }
}
