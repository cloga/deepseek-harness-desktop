import type { AppConfig } from '@/hooks/use-app-config'
import { Description, ListBox, Select } from '@heroui/react'
import { useMutation } from '@tanstack/react-query'
import { invoke } from '@tauri-apps/api/core'
import { useTranslation } from 'react-i18next'
import { useAppConfig } from '@/hooks/use-app-config'
import { toast } from '@/utils/toast'
import { CLOSE_ACTION_OPTIONS, normalizeCloseAction } from '../utils/close-action'

const CLOSE_ACTION_LABEL_KEYS = {
  tray: 'ui.close_action_tray',
  quit: 'ui.close_action_quit',
}

export function ConfigCloseAction() {
  const { t } = useTranslation()
  const { data: config, refetch, isFetching } = useAppConfig()
  const { mutate: setCloseAction, isPending } = useMutation({
    mutationFn: async (closeAction: string) => {
      const next = normalizeCloseAction(closeAction)
      await invoke<AppConfig>('update_app_config', { closeAction: next })
      await refetch()
    },
    onError: (error: unknown) => {
      console.error('[ConfigCloseAction] update failed:', error)
      toast(t('messages.close_action_failed'), { variant: 'danger' })
    },
  })

  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-medium text-ink">{t('ui.close_action')}</span>
        <Select
          variant="secondary"
          selectedKey={normalizeCloseAction(config?.close_action)}
          onSelectionChange={key => setCloseAction(String(key))}
          isDisabled={isFetching || isPending}
          className="w-[140px]"
          aria-label={t('ui.close_action')}
        >
          <Select.Trigger className="rounded-md min-h-8! h-8 py-0 items-center">
            <Select.Value />
            <Select.Indicator />
          </Select.Trigger>
          <Select.Popover className="rounded-md">
            <ListBox>
              {CLOSE_ACTION_OPTIONS.map(action => (
                <ListBox.Item
                  className="rounded-md min-h-8!"
                  id={action}
                  key={action}
                  textValue={t(CLOSE_ACTION_LABEL_KEYS[action])}
                >
                  {t(CLOSE_ACTION_LABEL_KEYS[action])}
                </ListBox.Item>
              ))}
            </ListBox>
          </Select.Popover>
        </Select>
      </div>
      <Description className="text-[10px] text-muted/70">
        {t('ui.close_action_hint')}
      </Description>
    </div>
  )
}
