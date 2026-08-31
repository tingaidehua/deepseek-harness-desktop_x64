window.__ModuleLoader__.load({
  id: 'dsh-desktop-control',
  factory: () => {
    const API = '/api/dsh-desktop-control'

    function sessionState(ctx) {
      const state = ctx.sessions.list.getSnapshot()
      const workspaces = ctx.workspaces.list.getSnapshot()
      const current = state.current
      const summary = current === undefined ? undefined : state.byId[current]
      const currentWorkspace = current === undefined
        ? undefined
        : workspaces.items.find(item => item.sessionIds.includes(current))
      return {
        sessionsPhase: state.phase,
        current: current ?? null,
        currentBlank: summary?.blank === true,
        currentCwd: summary?.cwd ?? null,
        currentWorkspaceId: currentWorkspace?.workspaceId ?? null,
        ids: [...state.ids],
        sessions: state.ids.map(id => ({
          id,
          blank: state.byId[id]?.blank === true,
          cwd: state.byId[id]?.cwd ?? null,
        })),
        workspacesPhase: workspaces.phase,
        archivedSessionIds: [...workspaces.archivedSessionIds],
        workspaces: workspaces.items.map(item => ({
          workspaceId: item.workspaceId,
          path: item.path,
          sessionIds: [...item.sessionIds],
        })),
      }
    }

    async function report(value) {
      await fetch(`${API}/report`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ ...value, observedAtMs: Date.now() }),
      })
    }

    function newSessionButton() {
      const labels = new Set(['新会话', 'New session'])
      return [...document.querySelectorAll('button')].find((button) => {
        const text = String(button.innerText || button.getAttribute('aria-label') || '').trim()
        return labels.has(text) && button.getClientRects().length > 0 && !button.disabled
      })
    }

    async function clickNewSession(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      const button = newSessionButton()
      if (button === undefined) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'NEW_SESSION_BUTTON_MISSING' })
        return
      }
      const uiWorkspace = ctx.workspaces
      const originalStartSession = uiWorkspace.startSession
      let currentServiceCalls = 0
      uiWorkspace.startSession = function (...args) {
        currentServiceCalls += 1
        return originalStartSession.apply(this, args)
      }
      try {
        button.click()
        await new Promise(resolve => queueMicrotask(resolve))
      }
      finally {
        uiWorkspace.startSession = originalStartSession
      }
      const buttonInfo = {
        text: String(button.innerText || '').trim(),
        ariaLabel: button.getAttribute('aria-label'),
        className: String(button.className),
        inSidebar: button.closest('[data-slot="sidebar"]') !== null,
      }
      await report({ id: command.id, action: command.action, phase: 'clicked', before, buttonInfo, currentServiceCalls })
      const deadline = Date.now() + 10_000
      while (Date.now() < deadline) {
        const after = sessionState(ctx)
        if (after.currentBlank && (before.currentBlank || after.current !== before.current)) {
          await report({ id: command.id, action: command.action, phase: 'completed', before, after, buttonInfo, currentServiceCalls })
          return
        }
        await new Promise(resolve => setTimeout(resolve, 100))
      }
      await report({ id: command.id, action: command.action, phase: 'failed', before, after: sessionState(ctx), buttonInfo, currentServiceCalls, error: 'NEW_SESSION_TRANSITION_TIMEOUT' })
    }

    async function waitForSelection(ctx, command, before, expectedId) {
      const deadline = Date.now() + 10_000
      while (Date.now() < deadline) {
        const after = sessionState(ctx)
        if (after.currentBlank && (before.currentBlank || after.current !== before.current)
          && (expectedId === undefined || after.current === expectedId)) {
          await report({ id: command.id, action: command.action, phase: 'completed', before, after, expectedId: expectedId ?? null })
          return
        }
        await new Promise(resolve => setTimeout(resolve, 100))
      }
      await report({ id: command.id, action: command.action, phase: 'failed', before, after: sessionState(ctx), expectedId: expectedId ?? null, error: 'SESSION_SELECTION_TIMEOUT' })
    }

    async function startCurrentWorkspace(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      if (before.currentWorkspaceId === null) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'CURRENT_WORKSPACE_MISSING' })
        return
      }
      ctx.workspaces.startSession(before.currentWorkspaceId)
      await report({ id: command.id, action: command.action, phase: 'invoked', before })
      await waitForSelection(ctx, command, before)
    }

    async function startUnscoped(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      ctx.workspaces.startSession()
      await report({ id: command.id, action: command.action, phase: 'invoked', before })
      await waitForSelection(ctx, command, before)
    }

    async function connectCurrentWorkspace(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      if (before.currentWorkspaceId === null) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'CURRENT_WORKSPACE_MISSING' })
        return
      }
      try {
        const sessionId = await ctx.workspaces.connectWorkspace(before.currentWorkspaceId)
        await report({ id: command.id, action: command.action, phase: 'connected', before, expectedId: sessionId })
        ctx.sessions.open(sessionId)
        await waitForSelection(ctx, command, before, sessionId)
      }
      catch (error) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, after: sessionState(ctx), error: error instanceof Error ? error.message : String(error) })
      }
    }

    async function openCurrentWorkspaceBlank(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      const workspace = before.workspaces.find(item => item.workspaceId === before.currentWorkspaceId)
      const blank = before.sessions.find(item => item.blank
        && (workspace === undefined || workspace.sessionIds.includes(item.id)))
      if (blank === undefined) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'CURRENT_WORKSPACE_BLANK_MISSING' })
        return
      }
      ctx.sessions.open(blank.id)
      await report({ id: command.id, action: command.action, phase: 'invoked', before, expectedId: blank.id })
      await waitForSelection(ctx, command, before, blank.id)
    }

    async function openNonblank(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      const target = before.sessions.find(item => !item.blank && item.id !== before.current)
      if (target === undefined) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'NONBLANK_SESSION_MISSING' })
        return
      }
      ctx.sessions.open(target.id)
      const after = sessionState(ctx)
      if (after.current === target.id && !after.currentBlank) {
        await report({ id: command.id, action: command.action, phase: 'completed', before, after, expectedId: target.id })
        return
      }
      await report({ id: command.id, action: command.action, phase: 'failed', before, after, expectedId: target.id, error: 'NONBLANK_SESSION_SELECTION_FAILED' })
    }

    function visibleElementByText(labels) {
      return [...document.querySelectorAll('button,[role="menuitem"]')].find((element) => {
        const text = String(element.innerText || element.getAttribute('aria-label') || '').trim()
        return labels.has(text) && element.getClientRects().length > 0
      })
    }

    async function clickArchiveCurrent(ctx, command) {
      const before = sessionState(ctx)
      await report({ id: command.id, action: command.action, phase: 'received', before })
      if (before.current === null || before.currentBlank) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'ARCHIVE_NONBLANK_CURRENT_REQUIRED' })
        return
      }
      const selected = document.querySelector('[role="treeitem"][aria-selected="true"]')
      const menuButton = selected?.querySelector('button[aria-label]')
      if (menuButton === undefined) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'ARCHIVE_MENU_BUTTON_MISSING' })
        return
      }
      menuButton.click()
      await new Promise(resolve => setTimeout(resolve, 100))
      const archiveAction = visibleElementByText(new Set(['归档会话', 'Archive session']))
      if (archiveAction === undefined) {
        await report({ id: command.id, action: command.action, phase: 'failed', before, error: 'ARCHIVE_ACTION_MISSING' })
        return
      }
      archiveAction.click()
      const archiveDeadline = Date.now() + 10_000
      while (Date.now() < archiveDeadline) {
        const archived = sessionState(ctx)
        if (archived.archivedSessionIds.includes(before.current)) {
          await report({ id: command.id, action: command.action, phase: 'archived', before, archived })
          const rollback = await fetch('/api/dsh-session/unarchive', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ sessionId: before.current }),
          })
          if (!rollback.ok) {
            await report({ id: command.id, action: command.action, phase: 'failed', before, archived, error: `ARCHIVE_ROLLBACK_HTTP_${rollback.status}` })
            return
          }
          const rollbackDeadline = Date.now() + 10_000
          while (Date.now() < rollbackDeadline) {
            const after = sessionState(ctx)
            if (!after.archivedSessionIds.includes(before.current)) {
              await report({ id: command.id, action: command.action, phase: 'completed', before, archived, after, rolledBack: true })
              return
            }
            await new Promise(resolve => setTimeout(resolve, 100))
          }
          await report({ id: command.id, action: command.action, phase: 'failed', before, archived, after: sessionState(ctx), error: 'ARCHIVE_ROLLBACK_TIMEOUT' })
          return
        }
        await new Promise(resolve => setTimeout(resolve, 100))
      }
      await report({ id: command.id, action: command.action, phase: 'failed', before, after: sessionState(ctx), error: 'ARCHIVE_STATE_TIMEOUT' })
    }

    function apply(ctx) {
      window.__dshDesktopControlStop?.()
      let stopped = false
      window.__dshDesktopControlStop = () => { stopped = true }
      let ready = false
      const run = async () => {
        while (!stopped) {
          try {
            if (!ready) {
              await report({ phase: 'ready', state: sessionState(ctx) })
              ready = true
            }
            const response = await fetch(`${API}/next`, { cache: 'no-store' })
            const body = await response.json()
            if (body.command?.action === 'session.click-new') await clickNewSession(ctx, body.command)
            else if (body.command?.action === 'session.start-unscoped') await startUnscoped(ctx, body.command)
            else if (body.command?.action === 'session.start-current-workspace') await startCurrentWorkspace(ctx, body.command)
            else if (body.command?.action === 'session.connect-current-workspace') await connectCurrentWorkspace(ctx, body.command)
            else if (body.command?.action === 'session.open-current-workspace-blank') await openCurrentWorkspaceBlank(ctx, body.command)
            else if (body.command?.action === 'session.open-nonblank') await openNonblank(ctx, body.command)
            else if (body.command?.action === 'session.click-archive') await clickArchiveCurrent(ctx, body.command)
          }
          catch (error) {
            await report({ phase: 'probe-error', error: error instanceof Error ? error.message : String(error) }).catch(() => {})
          }
          await new Promise(resolve => setTimeout(resolve, 100))
        }
      }
      void run()
    }

    return { apply, inject: ['sessions', 'workspaces'], name: 'dsh-desktop-control' }
  },
})
