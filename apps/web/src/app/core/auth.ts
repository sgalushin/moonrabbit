import { Injectable, inject, signal } from '@angular/core';
import type { CanActivateFn } from '@angular/router';
import { UserManager, WebStorageStateStore } from 'oidc-client-ts';

interface AuthSettings {
  authority: string;
  clientId: string;
  domain: string;
}

interface AppConfig {
  auth: AuthSettings | null;
}

@Injectable({ providedIn: 'root' })
export class Auth {
  readonly enabled = signal(false);
  readonly email = signal<string | null>(null);
  private manager: UserManager | null = null;
  private config: AppConfig | null = null;
  private loading: Promise<void> | null = null;

  ready(): Promise<void> {
    this.loading ??= this.load();
    return this.loading;
  }

  async login(): Promise<void> {
    await this.ready();
    await this.manager?.signinRedirect();
  }

  async handleCallback(): Promise<void> {
    await this.ready();
    if (!this.manager) {
      return;
    }
    const user = await this.manager.signinRedirectCallback();
    this.email.set(user.profile.email ?? null);
  }

  async token(): Promise<string | null> {
    await this.ready();
    if (!this.manager) {
      return null;
    }
    let user = await this.manager.getUser();
    if (user?.expired) {
      try {
        user = await this.manager.signinSilent();
      } catch {
        return null;
      }
    }
    return user?.access_token ?? null;
  }

  async signOut(): Promise<void> {
    await this.ready();
    const auth = this.config?.auth;
    await this.manager?.removeUser();
    if (!auth) {
      return;
    }
    const logout = new URL('/logout', auth.domain);
    logout.searchParams.set('client_id', auth.clientId);
    logout.searchParams.set('logout_uri', `${location.origin}/`);
    location.assign(logout.toString());
  }

  private async load(): Promise<void> {
    const response = await fetch('/config.json');
    this.config = (await response.json()) as AppConfig;
    const auth = this.config.auth;
    this.enabled.set(auth != null);
    if (!auth) {
      return;
    }
    this.manager = new UserManager({
      authority: auth.authority,
      client_id: auth.clientId,
      redirect_uri: `${location.origin}/auth/callback`,
      post_logout_redirect_uri: `${location.origin}/`,
      response_type: 'code',
      scope: 'openid email profile',
      automaticSilentRenew: true,
      userStore: new WebStorageStateStore({ store: localStorage }),
    });
    this.manager.events.addUserLoaded((user) => this.email.set(user.profile.email ?? null));
    this.manager.events.addUserUnloaded(() => this.email.set(null));
    const user = await this.manager.getUser();
    this.email.set(user?.profile.email ?? null);
  }
}

export const authGuard: CanActivateFn = async () => {
  const auth = inject(Auth);
  await auth.ready();
  if (!auth.enabled()) {
    return true;
  }
  if (await auth.token()) {
    return true;
  }
  await auth.login();
  return false;
};
