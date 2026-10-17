import {
  Component,
  DestroyRef,
  ElementRef,
  EventEmitter,
  Output,
  ViewChild,
  inject,
  computed,
  signal,
} from '@angular/core';
import { CardPhase, CardStat, ListWord } from '@moonrabbit/contract';
import { LucideX } from '@lucide/angular';
import { Api } from '../core/api';
import { apiErrorMessage } from '../core/errors';
import { Pinyin } from '../pinyin';
import { AudioCache } from '../core/audio';
import { WordExamples } from './word-examples';

const CARD_LABELS: Record<CardPhase, string> = {
  new: 'Not studied',
  learning: 'Learning',
  relearning: 'Relearning',
  review: 'Review',
};

@Component({
  selector: 'app-word-detail',
  imports: [Pinyin, LucideX, WordExamples],
  templateUrl: './word-detail.html',
})
export class WordDetail {
  private api = inject(Api);
  private destroyRef = inject(DestroyRef);
  private audio = inject(AudioCache);
  private wordId = signal<string | undefined>(undefined);
  readonly wordResource = this.api.word(() => this.wordId());
  private previousFocus?: HTMLElement;
  @ViewChild('wordDialog', { static: true }) dialog?: ElementRef<HTMLDialogElement>;
  @Output() closed = new EventEmitter<void>();
  readonly heading = signal('Word details');
  readonly detail = computed(() =>
    this.wordResource.hasValue() ? this.wordResource.value() : null,
  );
  readonly loading = this.wordResource.isLoading;
  readonly error = computed(() =>
    this.wordResource.error()
      ? apiErrorMessage(
          this.wordResource.error(),
          'This word’s review details could not be loaded.',
        )
      : '',
  );

  constructor() {
    this.destroyRef.onDestroy(() => this.close());
  }

  open(word: ListWord) {
    if (!this.dialog?.nativeElement.open) {
      this.previousFocus =
        document.activeElement instanceof HTMLElement ? document.activeElement : undefined;
    }
    this.heading.set(word.simplified);
    this.audio.stop();
    const dialog = this.dialog?.nativeElement;
    if (dialog && !dialog.open) dialog.showModal();
    this.load(word.id);
  }

  load(wordId: string) {
    if (this.wordId() === wordId) this.wordResource.reload();
    else this.wordId.set(wordId);
  }

  close() {
    this.wordId.set(undefined);
    this.audio.stop();
    this.dialog?.nativeElement.close();
    this.previousFocus?.focus();
    this.previousFocus = undefined;
    this.closed.emit();
  }

  containFocus(event: KeyboardEvent) {
    if (event.key !== 'Tab') return;
    const dialog = this.dialog?.nativeElement;
    if (!dialog) return;
    const controls = [
      ...dialog.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])',
      ),
    ].filter((control) => control.getClientRects().length > 0);
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (
      first &&
      ((event.shiftKey && document.activeElement === first) ||
        (!event.shiftKey && document.activeElement === last))
    ) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  }

  when(millis: number | undefined) {
    if (!millis) return '—';
    return new Date(millis).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    });
  }

  stateLabel(card: CardStat) {
    if (!card.studied) return 'Not studied';
    return CARD_LABELS[card.state];
  }
}
