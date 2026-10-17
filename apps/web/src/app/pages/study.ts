import { Component, DestroyRef, computed, effect, inject, signal, untracked } from '@angular/core';
import { RouterLink } from '@angular/router';
import {
  Direction,
  Face,
  Rating,
  SESSION_AMOUNTS,
  SessionRequest,
  SessionUnit,
} from '@moonrabbit/contract';
import { AudioCache } from '../core/audio';
import { Grades } from '../core/grades';
import { Api } from '../core/api';
import { apiErrorMessage } from '../core/errors';
import { GradeNotice } from '../core/grade-notice';
import { LucideLoaderCircle, LucideVolume2 } from '@lucide/angular';
import { Pinyin } from '../pinyin';
import { Shell } from '../shell';
import { WordExamples } from './word-examples';

type Phase = 'launch' | 'prompt' | 'reveal' | 'answer' | 'summary';

@Component({
  selector: 'app-study',
  imports: [
    RouterLink,
    Shell,
    Pinyin,
    GradeNotice,
    LucideVolume2,
    LucideLoaderCircle,
    WordExamples,
  ],
  templateUrl: './study.html',
})
export class StudyPage {
  private api = inject(Api);
  private audio = inject(AudioCache);
  private destroyRef = inject(DestroyRef);
  readonly grades = inject(Grades);
  phase = signal<Phase>('launch');
  unit = signal<SessionUnit>('words');
  amount = signal(2);
  direction = signal<Direction>('recognition');
  cards = signal<Face[]>([]);
  index = signal(0);
  deadline = 0;
  remaining = signal(0);
  expired = false;
  good = signal(0);
  again = signal(0);
  error = signal('');
  playing = signal(false);
  readonly presets = SESSION_AMOUNTS;
  launching = signal(false);
  recording = signal(false);
  sessionUnit: SessionUnit = 'words';
  private destroyed = false;
  private timer = 0;
  private launchRequest = signal<SessionRequest | undefined>(undefined);
  private appliedLaunch?: SessionRequest;
  readonly sessionResource = this.api.session(() => this.launchRequest());
  readonly wordResource = this.api.word(() =>
    this.phase() === 'reveal' || this.phase() === 'answer' ? this.face()?.wordId : undefined,
  );
  readonly exampleWord = computed(() =>
    this.wordResource.hasValue() ? this.wordResource.value() : undefined,
  );

  constructor() {
    this.destroyRef.onDestroy(() => {
      this.destroyed = true;
      window.clearInterval(this.timer);
      this.audio.stop();
    });
    effect(() => {
      const launch = this.launchRequest();
      const status = this.sessionResource.status();
      if (!launch) return;
      if (status === 'error') {
        const error = this.sessionResource.error();
        untracked(() => {
          if (this.appliedLaunch === launch) return;
          this.launching.set(false);
          this.error.set(
            apiErrorMessage(error, 'The session could not start.') +
              ' Your settings are unchanged; try Start again.',
          );
        });
      } else if (status === 'resolved' && this.sessionResource.hasValue()) {
        const session = this.sessionResource.value();
        // Only the resource completion owns initialization. Reading progress here
        // would make later answers rerun this effect and reset the session.
        untracked(() => {
          if (!this.launching() || this.appliedLaunch === launch) return;
          this.appliedLaunch = launch;
          this.launching.set(false);
          this.sessionUnit = launch.unit;
          this.cards.set(session.cards);
          this.index.set(0);
          this.good.set(0);
          this.again.set(0);
          this.expired = false;
          this.remaining.set(0);
          if (!session.cards.length) {
            this.phase.set('summary');
            return;
          }
          if (launch.unit === 'minutes') {
            this.deadline = Date.now() + launch.amount * 60_000;
            this.remaining.set(launch.amount * 60);
            this.timer = window.setInterval(() => {
              const left = Math.max(0, Math.ceil((this.deadline - Date.now()) / 1000));
              this.remaining.set(left);
              if (left === 0) {
                this.expired = true;
                window.clearInterval(this.timer);
                if (!this.recording()) this.finish();
              }
            }, 250);
          }
          this.phase.set('prompt');
        });
      }
    });
  }

  face() {
    return this.cards()[this.index()];
  }

  start() {
    if (this.launching() || this.phase() !== 'launch') return;
    const launch: SessionRequest = {
      unit: this.unit(),
      amount: this.amount(),
      direction: this.direction(),
    };
    this.launching.set(true);
    this.error.set('');
    window.clearInterval(this.timer);
    this.launchRequest.set(launch);
  }

  async know(yes: boolean) {
    if (this.recording() || this.phase() !== 'prompt') return;
    if (yes) {
      this.phase.set('reveal');
    } else if (await this.record('again')) {
      if (this.expired) this.finish();
      else this.phase.set('answer');
    }
  }

  async correct(yes: boolean) {
    if (this.recording() || this.phase() !== 'reveal') return;
    if (await this.record(yes ? 'good' : 'again')) this.advance();
  }

  next() {
    if (!this.recording() && this.phase() === 'answer') this.advance();
  }

  finish() {
    if (this.recording()) return;
    window.clearInterval(this.timer);
    this.audio.stop();
    this.phase.set('summary');
  }

  async play() {
    const face = this.face();
    if (!face) return;
    this.playing.set(true);
    try {
      await this.audio.play(face);
    } catch (error: unknown) {
      this.error.set(apiErrorMessage(error, 'This word could not be played.'));
    } finally {
      this.playing.set(false);
    }
  }

  clock() {
    const seconds = this.remaining();
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
  }

  private async record(rating: Rating): Promise<boolean> {
    const face = this.face();
    if (!face || this.recording()) return false;
    this.recording.set(true);
    try {
      await this.grades.enqueue({
        eventId: crypto.randomUUID(),
        wordId: face.wordId,
        direction: face.direction,
        rating,
        reviewedAt: Date.now(),
      });
      if (this.destroyed) return false;
      if (rating === 'good') this.good.update((count) => count + 1);
      else this.again.update((count) => count + 1);
      this.error.set('');
      return true;
    } catch {
      if (!this.destroyed)
        this.error.set(
          'Your answer could not be stored on this device. It has not been counted. Free storage or resolve the grade-queue error below, then answer again.',
        );
      return false;
    } finally {
      this.recording.set(false);
    }
  }

  private advance() {
    this.audio.stop();
    const next = this.index() + 1;
    if (this.expired || next >= this.cards().length) {
      this.finish();
      return;
    }
    this.index.set(next);
    this.phase.set('prompt');
  }
}
