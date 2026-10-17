import { Component, inject } from '@angular/core';
import { Grades } from './grades';

@Component({
  selector: 'app-grade-notice',
  template: `
    @if (grades.failed()) {
      <p class="banner" role="alert">
        {{ grades.error() }}
        <button type="button" (click)="grades.flush()">Retry sending stored answers</button>
        <button type="button" (click)="grades.exportPending()">Download pending answers</button>
      </p>
    }
  `,
})
export class GradeNotice {
  readonly grades = inject(Grades);
}
