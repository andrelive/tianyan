import { test, expect } from '@playwright/test';

test.describe('chat', () => {
  test.beforeEach(async ({ page }) => {
    // Mock config status to skip ConfigWizard
    await page.route('**/api/v1/config/status', async (route) => {
      await route.fulfill({ json: { configured: true } });
    });

    // Mock sessions list (needed because sidebar loads sessions on mount)
    await page.route('**/api/v1/sessions', async (route) => {
      await route.fulfill({
        json: {
          sessions: [
            {
              id: 's1',
              title: '测试会话',
              created_at: '2026-07-20T10:00:00Z',
              updated_at: '2026-07-23T08:00:00Z',
              message_count: 5,
            },
          ],
          total: 1,
        },
      });
    });

    // Mock chat stream endpoint with SSE-formatted response
    await page.route('**/api/v1/chat/stream', async (route) => {
      const ssePayload = [
        'data: ' +
          JSON.stringify({
            id: 'm1',
            session_id: 's1',
            delta: '你好！有什么我可以帮助你的吗？',
            finish_reason: null,
            skill_calls: null,
            chunk_type: 'answer',
          }),
        '',
        'data: [DONE]',
        '',
      ].join('\n');

      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body: ssePayload,
      });
    });

    // Navigate to app (redirects to /chat)
    await page.goto('/');
  });

  test('empty state shows placeholder when no messages', async ({ page }) => {
    // Chat panel should show empty state
    await expect(page.getByText('开始一段新的对话')).toBeVisible();
  });

  test('chat input is focused on page load', async ({ page }) => {
    const textarea = page.locator('textarea');
    await expect(textarea).toBeFocused();
  });

  test('typing message and sending with Enter', async ({ page }) => {
    const textarea = page.locator('textarea');
    await expect(textarea).toBeVisible();

    // Type a message
    await textarea.fill('你好');

    // Press Enter to send
    await textarea.press('Enter');

    // User message should appear in chat
    await expect(page.getByText('你好')).toBeVisible();
  });

  test('assistant response appears after sending message', async ({ page }) => {
    const textarea = page.locator('textarea');

    // Send a message
    await textarea.fill('帮我介绍一下天演');
    await textarea.press('Enter');

    // Wait for streaming response to complete
    await expect(
      page.getByText('你好！有什么我可以帮助你的吗？')
    ).toBeVisible({ timeout: 10000 });
  });

  test('send button is enabled when input has text', async ({ page }) => {
    const textarea = page.locator('textarea');
    const sendButton = page.locator('button', { hasText: '发送' });

    // Send button should be disabled initially (no text)
    await expect(sendButton).toBeDisabled();

    // Type some text
    await textarea.fill('测试消息');

    // Send button should now be enabled
    await expect(sendButton).toBeEnabled();
  });
});
