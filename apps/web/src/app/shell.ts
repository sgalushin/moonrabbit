import { Component, inject } from '@angular/core';
import { RouterLink, RouterLinkActive } from '@angular/router';
import { LucideHouse, LucideListOrdered, LucidePlay } from '@lucide/angular';
import { Auth } from './core/auth';
import { Grades } from './core/grades';
import { GradeNotice } from './core/grade-notice';
import { Logo } from './logo';

@Component({
  selector: 'app-shell',
  imports: [
    RouterLink,
    RouterLinkActive,
    Logo,
    GradeNotice,
    LucideHouse,
    LucideListOrdered,
    LucidePlay,
  ],
  template: `
    <div class="library">
      <header class="library-head">
        <a class="brand" routerLink="/">
          <app-logo />
          <h1>Moon <span class="sage-text">Rabbit</span></h1>
        </a>
        <nav class="library-actions" aria-label="Main">
          <a
            routerLink="/"
            aria-label="Home"
            routerLinkActive="active"
            [routerLinkActiveOptions]="{ exact: true }"
          >
            <svg lucideHouse [size]="18" aria-hidden="true"></svg>
            <span>Home</span>
          </a>
          <a routerLink="/queue" aria-label="Queue" routerLinkActive="active">
            <svg lucideListOrdered [size]="18" aria-hidden="true"></svg>
            <span>Queue</span>
          </a>
          <a routerLink="/study" aria-label="Study" routerLinkActive="active">
            <svg lucidePlay [size]="18" aria-hidden="true"></svg>
            <span>Study</span>
          </a>
        </nav>
        @if (auth.enabled()) {
          <button type="button" class="ghost sign-out" (click)="signOut()">Sign out</button>
        }
      </header>
      <app-grade-notice />
      <ng-content />
    </div>
  `,
})
export class Shell {
  grades = inject(Grades);
  auth = inject(Auth);

  constructor() {
    void this.grades.flush();
    void this.auth.ready();
  }

  signOut() {
    void this.auth.signOut();
  }
}
