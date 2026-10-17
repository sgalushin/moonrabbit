import { Component, DestroyRef, EventEmitter, Input, Output, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { ExampleSentence, WordDetail } from '@moonrabbit/contract';
import { LucideLoaderCircle, LucideVolume2 } from '@lucide/angular';
import { Api } from '../core/api';
import { AudioCache } from '../core/audio';
import { apiErrorMessage } from '../core/errors';
import { Pinyin } from '../pinyin';

@Component({
  selector: 'app-word-examples',
  imports: [Pinyin, LucideLoaderCircle, LucideVolume2],
  template: `
    @if (wordVoice) {
      <button
        class="ghost"
        type="button"
        (click)="play()"
        [attr.aria-label]="'Play ' + word.simplified"
      >
        <svg lucideVolume2 [size]="20" aria-hidden="true"></svg> Play word
      </button>
    }
    <section class="examples" aria-label="Example sentences">
      <h3>Example sentences</h3>
      @for (sentence of word.examples; track sentence.id) {
        <article class="example">
          <p class="chars">{{ sentence.simplified }}</p>
          <p><span class="label">formal pinyin</span> <app-pinyin [text]="sentence.pinyin" /></p>
          <p>
            <span class="label">pronounced as</span>
            <app-pinyin [text]="sentence.pronouncedPinyin" />
          </p>
          <p>{{ sentence.translation }}</p>
          <button
            class="ghost"
            type="button"
            (click)="play(sentence)"
            [attr.aria-label]="'Play sentence ' + sentence.simplified"
          >
            @if (playing() === sentence.id) {
              <svg lucideLoaderCircle [size]="20" class="spin" aria-hidden="true"></svg>
            } @else {
              <svg lucideVolume2 [size]="20" aria-hidden="true"></svg>
            }
            Play sentence
          </button>
        </article>
      } @empty {
        <p class="help">No saved examples yet.</p>
      }
      <button
        class="ghost"
        type="button"
        [disabled]="generating().has(word.id)"
        (click)="generate()"
      >
        {{
          generating().has(word.id) ? 'Generating and saving…' : 'Generate and save HSK1 example'
        }}
      </button>
      <p class="help">
        Generated sentences use HSK1 characters and this word. Words outside HSK1 cannot receive a
        constrained example. Screenshot source sentences are preserved as written.
      </p>
      @if (errors().get(word.id); as message) {
        <p class="banner" role="alert">{{ message }}</p>
      }
    </section>
  `,
})
export class WordExamples {
  private api = inject(Api);
  private audio = inject(AudioCache);
  private destroyRef = inject(DestroyRef);
  @Input({ required: true }) word!: WordDetail;
  @Input() wordVoice = true;
  @Output() changed = new EventEmitter<WordDetail>();
  readonly generating = signal(new Set<string>());
  readonly errors = signal(new Map<string, string>());
  readonly playing = signal<string | null>(null);
  private playback = 0;
  private destroyed = false;

  constructor() {
    this.destroyRef.onDestroy(() => {
      this.destroyed = true;
      this.playback++;
      this.audio.stop();
    });
  }

  generate() {
    const id = this.word.id;
    if (this.generating().has(id)) return;
    this.generating.update((pending) => new Set(pending).add(id));
    this.errors.update((errors) => {
      const next = new Map(errors);
      next.delete(id);
      return next;
    });
    this.api
      .generateExample(id)
      .pipe(takeUntilDestroyed(this.destroyRef))
      .subscribe({
        next: (word) => {
          this.generating.update((pending) => {
            const next = new Set(pending);
            next.delete(id);
            return next;
          });
          if (this.word.id === id) this.changed.emit(word);
        },
        error: (error: unknown) => {
          this.generating.update((pending) => {
            const next = new Set(pending);
            next.delete(id);
            return next;
          });
          this.errors.update((errors) =>
            new Map(errors).set(
              id,
              apiErrorMessage(error, 'An example could not be generated and saved. Try again.'),
            ),
          );
        },
      });
  }

  async play(sentence?: ExampleSentence) {
    const word = this.word;
    const generation = ++this.playback;
    this.playing.set(sentence?.id ?? word.id);
    try {
      await this.audio.play(
        {
          wordId: word.id,
          direction: 'recognition',
          simplified: sentence?.simplified ?? word.simplified,
          pinyin: sentence?.pinyin ?? word.pinyin,
          pronouncedPinyin: sentence?.pronouncedPinyin ?? word.pronouncedPinyin,
          translation: sentence?.translation ?? word.translation,
        },
        sentence?.id,
      );
    } catch (error: unknown) {
      if (!this.destroyed && generation === this.playback && word.id === this.word.id)
        this.errors.update((errors) =>
          new Map(errors).set(
            word.id,
            apiErrorMessage(error, 'Audio could not be played. Try again.'),
          ),
        );
    } finally {
      if (generation === this.playback) this.playing.set(null);
    }
  }
}
