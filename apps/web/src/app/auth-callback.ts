import { Component, inject } from '@angular/core';
import { Router } from '@angular/router';
import { Auth } from './core/auth';

@Component({
  selector: 'app-auth-callback',
  template: `<p class="status">Signing in…</p>`,
})
export class AuthCallbackPage {
  private readonly auth = inject(Auth);
  private readonly router = inject(Router);

  constructor() {
    void this.finish();
  }

  private async finish(): Promise<void> {
    try {
      await this.auth.handleCallback();
      await this.router.navigateByUrl('/');
    } catch {
      await this.auth.login();
    }
  }
}
