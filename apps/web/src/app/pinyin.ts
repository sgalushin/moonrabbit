import { Component, Input } from '@angular/core';
import { toneOf } from './core/pinyin';

@Component({
  selector: 'app-pinyin',
  template: `
    <span class="pinyin">
      @for (part of parts; track $index) {
        <i [class]="'tone-' + part.tone">{{ part.text }}</i>
      }
    </span>
  `,
})
export class Pinyin {
  parts: { text: string; tone: number }[] = [];
  @Input({ required: true }) set text(value: string) {
    this.parts = value
      .split(/\s+/)
      .filter(Boolean)
      .map((text) => ({ text, tone: toneOf(text) }));
  }
}
