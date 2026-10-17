import { Route } from '@angular/router';
import { authGuard } from './core/auth';

export const appRoutes: Route[] = [
  {
    path: 'auth/callback',
    loadComponent: () => import('./auth-callback').then((module) => module.AuthCallbackPage),
  },
  {
    path: '',
    canActivate: [authGuard],
    children: [
      { path: '', loadComponent: () => import('./pages/home').then((module) => module.HomePage) },
      {
        path: 'queue',
        loadComponent: () => import('./pages/queue').then((module) => module.QueuePage),
      },
      {
        path: 'lists/:id',
        loadComponent: () => import('./pages/list').then((module) => module.ListPage),
      },
      {
        path: 'study',
        loadComponent: () => import('./pages/study').then((module) => module.StudyPage),
      },
    ],
  },
  { path: '**', redirectTo: '' },
];
