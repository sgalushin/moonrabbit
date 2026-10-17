import { Injectable, OnDestroy, inject } from '@angular/core';
import { firstValueFrom } from 'rxjs';
import { Face } from '@moonrabbit/contract';
import { Api } from './api';

const INDEX = 'moonrabbit.audio.index.v2';
const PREFIX = 'moonrabbit.audio.v2.';
const BUDGET = 4_000_000;

@Injectable({ providedIn: 'root' })
export class AudioCache implements OnDestroy {
  private api = inject(Api);
  private playing: { audio: HTMLAudioElement; url: string } | null = null;
  private generation = 0;

  async play(face: Face, sentenceId?: string): Promise<void> {
    const generation = ++this.generation;
    this.release();
    const key = `${PREFIX}${face.wordId}|${sentenceId ? `sentence:${sentenceId}|` : ''}${face.pronouncedPinyin ?? face.pinyin}`;
    let buffer = this.read(key);
    const cached = buffer !== null;
    if (!buffer) {
      try {
        buffer = await firstValueFrom(this.api.speech(face.wordId, sentenceId));
      } catch (error) {
        if (generation !== this.generation) return;
        throw error;
      }
      if (generation !== this.generation) return;
      this.store(key, new Uint8Array(buffer));
    }
    if (generation !== this.generation) return;
    const url = URL.createObjectURL(new Blob([buffer], { type: 'audio/mpeg' }));
    let audio: HTMLAudioElement;
    try {
      audio = new Audio(url);
    } catch (error) {
      URL.revokeObjectURL(url);
      throw error;
    }
    const playing = { audio, url };
    let mediaFailed = false;
    this.playing = playing;
    audio.onended = () => {
      if (this.playing === playing) this.release();
    };
    audio.onerror = () => {
      mediaFailed = true;
      this.remove(key);
      if (this.playing === playing) this.release();
    };
    try {
      await audio.play();
    } catch (error) {
      const corruptCache = cached && (mediaFailed || !!audio.error);
      if (this.playing === playing) this.release();
      if (generation !== this.generation) return;
      if (corruptCache) {
        this.remove(key);
        return this.play(face, sentenceId);
      }
      throw error;
    }
  }

  stop(): void {
    this.generation += 1;
    this.release();
  }

  ngOnDestroy(): void {
    this.stop();
  }

  private release(): void {
    const playing = this.playing;
    if (!playing) return;
    this.playing = null;
    playing.audio.onended = null;
    playing.audio.onerror = null;
    try {
      playing.audio.pause();
      playing.audio.removeAttribute('src');
      playing.audio.load();
    } finally {
      URL.revokeObjectURL(playing.url);
    }
  }

  private read(key: string): ArrayBuffer | null {
    try {
      const stored = localStorage.getItem(key);
      if (!stored) return null;
      const binary = atob(stored);
      const bytes = new Uint8Array(binary.length);
      for (let index = 0; index < binary.length; index += 1)
        bytes[index] = binary.charCodeAt(index);
      // Reject malformed cached MP3s before they can replace playable fetched audio.
      const id3 = bytes[0] === 0x49 && bytes[1] === 0x44 && bytes[2] === 0x33;
      const frame = bytes[0] === 0xff && (bytes[1] & 0xe0) === 0xe0;
      if (!id3 && !frame) {
        this.remove(key);
        return null;
      }
      return bytes.buffer;
    } catch {
      this.remove(key);
      return null;
    }
  }

  private store(key: string, bytes: Uint8Array): void {
    // Browser storage is an optional cache, never a prerequisite for playback.
    try {
      let binary = '';
      for (const byte of bytes) binary += String.fromCharCode(byte);
      const encoded = btoa(binary);
      if (encoded.length > BUDGET) return;
      const index = this.index().filter((item) => item !== key);
      let size = index.reduce((sum, item) => sum + (localStorage.getItem(item)?.length ?? 0), 0);
      while (size + encoded.length > BUDGET && index.length) {
        const oldest = index.shift();
        if (oldest === undefined) break;
        size -= localStorage.getItem(oldest)?.length ?? 0;
        localStorage.removeItem(oldest);
      }
      for (;;) {
        try {
          localStorage.setItem(key, encoded);
          localStorage.setItem(INDEX, JSON.stringify([...index, key]));
          return;
        } catch {
          localStorage.removeItem(key);
          const oldest = index.shift();
          if (!oldest) return;
          localStorage.removeItem(oldest);
        }
      }
    } catch {
      // Quota, corruption, and disabled storage must not suppress usable audio.
    }
  }

  private remove(key: string): void {
    try {
      localStorage.removeItem(key);
    } catch {
      // Storage may be disabled; cleanup remains best-effort.
    }
  }

  private index(): string[] {
    const parsed: unknown = JSON.parse(localStorage.getItem(INDEX) || '[]');
    return Array.isArray(parsed)
      ? [
          ...new Set(
            parsed.filter(
              (item): item is string => typeof item === 'string' && item.startsWith(PREFIX),
            ),
          ),
        ]
      : [];
  }
}
