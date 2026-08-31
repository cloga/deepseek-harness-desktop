import { ArrowLeft, Delete } from '@gravity-ui/icons'
import { Button, Checkbox, Chip, Description, Input, Label, Spinner, Switch } from '@heroui/react'
import { useOverlay } from '@overlastic/react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { If } from 'react-if-lite'
import { store } from '@/store'
import { toast } from '@/utils/toast'
import { useBackups } from '../hooks/use-backup'
import { normalizeIntervalDays, normalizeRetentionCount } from '../utils/backup-settings'
import { useAppConfig } from './../hooks/use-app-config'
import { Item } from './item'
import { Modal } from './modal'
import { PanelHeader } from './panel-header'
import { PanelState } from './panel-state'

export interface ConfigBackupProps {
  onBack: () => void
}

/** 把字节转为 MB 展示（保留 1 位小数）。 */
function formatSize(bytes: number): string {
  return `${(bytes / 1024 / 1024).toFixed(1)}`
}

export function ConfigBackup({ onBack }: ConfigBackupProps) {
  const { t } = useTranslation()
  const { backups, loading, error, createBackup, restoreBackup, deleteBackup, updateAutoBackupSettings, busy } = useBackups()
  const { data: config } = useAppConfig()
  const [dialogHolder, openDialog] = useOverlay(Modal, { type: 'holder' })

  const [includeCredentials, setIncludeCredentials] = useState(false)

  async function handleCreate() {
    try {
      await createBackup(includeCredentials)
      toast(t('backup.created_toast'), { variant: 'accent', timeout: 5000 })
    }
    catch (err) {
      console.error('[ConfigBackup] create failed:', err)
      toast(t('backup.failed_toast'), { variant: 'danger' })
    }
  }

  async function handleRestore(timestamp: string) {
    try {
      await openDialog({
        status: 'danger',
        title: t('backup.restore_confirm_title'),
        description: (
          <p>
            {t('backup.restore_confirm_desc', { timestamp })}
          </p>
        ),
      })
    }
    catch {
      return
    }
    try {
      toast(t('backup.restored_stopped_toast'), { variant: 'accent' })
      await restoreBackup(timestamp, false)
      const key = toast(t('backup.restored_toast'), {
        variant: 'accent',
        timeout: 10_000,
        actionProps: {
          children: t('app.restart'),
          onPress: () => {
            store.harness.restart()
            toast.close(key)
          },
        },
      })
    }
    catch (err) {
      console.error('[ConfigBackup] restore failed:', err)
      toast(t('backup.restore_failed'), { variant: 'danger' })
    }
  }

  async function handleRestoreAsNew(timestamp: string) {
    try {
      await openDialog({
        status: 'warning',
        title: t('backup.restore_new_confirm_title'),
        description: (
          <p>
            {t('backup.restore_new_confirm_desc', { timestamp })}
          </p>
        ),
      })
    }
    catch {
      return
    }
    try {
      await restoreBackup(timestamp, true)
      toast(t('backup.restored_toast'), { variant: 'accent', timeout: 5000 })
    }
    catch (err) {
      console.error('[ConfigBackup] restore as new failed:', err)
      toast(t('backup.restore_failed'), { variant: 'danger' })
    }
  }

  async function handleDelete(timestamp: string) {
    try {
      await openDialog({
        status: 'danger',
        title: t('backup.delete_confirm_title'),
        description: (
          <p>
            {t('backup.delete_confirm_desc', { timestamp })}
          </p>
        ),
        confirmText: t('backup.delete'),
      })
    }
    catch {
      return
    }
    try {
      await deleteBackup(timestamp)
      toast(t('backup.deleted_toast'), { variant: 'accent', timeout: 5000 })
    }
    catch (err) {
      console.error('[ConfigBackup] delete failed:', err)
      toast(t('backup.delete_failed'), { variant: 'danger' })
    }
  }

  return (
    <div className="space-y-3">
      <Button variant="tertiary" className="h-8 rounded-md" onPress={onBack}>
        <ArrowLeft className="size-3.5" />
        <span>{t('backup.back_to_profiles')}</span>
      </Button>

      {/* 手动备份 */}
      <PanelHeader title={t('backup.manual_section')} description="" />
      <div className="flex flex-col gap-3">
        <Button
          variant="primary"
          className="rounded-md"
          isDisabled={busy}
          onPress={handleCreate}
        >
          <If cond={busy}>
            <Spinner size="sm" color="current" />
            <span>{t('backup.in_progress')}</span>
          </If>
          <If cond={!busy}>
            <span>{t('backup.now')}</span>
          </If>
        </Button>
        <Checkbox
          isSelected={includeCredentials}
          onChange={(value: boolean) => setIncludeCredentials(value)}
          aria-label={t('backup.include_credentials')}
          className="shrink-0"
        >
          <Checkbox.Content>
            <Checkbox.Control>
              <Checkbox.Indicator />
            </Checkbox.Control>
          </Checkbox.Content>
        </Checkbox>
        <span className="text-xs text-ink">{t('backup.include_credentials')}</span>
        <If cond={includeCredentials}>
          <Description className="text-[10px] text-danger">
            {t('backup.credentials_warning')}
          </Description>
        </If>
      </div>

      {/* 备份列表 */}
      <PanelHeader title={t('backup.list_section')} description="" />
      <PanelState loading={loading} error={error}>
        <If
          cond={backups.length === 0}
          else={(
            <div className="flex flex-col gap-4">
              {backups.map(backup => (
                <Item
                  key={backup.timestamp}
                  left={(
                    <>
                      <Label className="text-xs font-mono text-muted">
                        {backup.timestamp}
                      </Label>
                      <Description className="text-xs font-mono text-muted">
                        {formatSize(backup.size)}
                        {' '}
                        {t('backup.size_unit')}
                      </Description>
                    </>
                  )}
                  right={(
                    <>
                      <Button size="sm" variant="tertiary" className="h-7 rounded-md" onPress={() => handleRestore(backup.timestamp)}>
                        {t('backup.restore')}
                      </Button>
                      <Button size="sm" variant="tertiary" className="h-7 rounded-md" onPress={() => handleRestoreAsNew(backup.timestamp)}>
                        {t('backup.restore_as_new')}
                      </Button>
                      <Chip
                        className="rounded-md"
                        variant="primary"
                        color="danger"
                        size="sm"
                        onClick={() => handleDelete(backup.timestamp)}
                      >
                        <Delete className="size-3" />
                      </Chip>
                    </>
                  )}
                />
              ))}
            </div>
          )}
        >
          <div className="space-y-1 rounded-md border border-line bg-panel2/40 p-3">
            <Label className="text-xs font-medium text-ink">{t('backup.empty_title')}</Label>
            <Description className="text-xs text-muted">{t('backup.empty_desc')}</Description>
          </div>
        </If>
      </PanelState>

      {/* 自动备份设置 */}
      <PanelHeader title={t('backup.auto_section')} description="" />
      <div className="flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-ink">{t('backup.auto_enable')}</span>
          <Switch
            isSelected={config?.auto_backup_enabled ?? false}
            onChange={value => updateAutoBackupSettings({ autoBackupEnabled: value })}
            size="sm"
          >
            <Switch.Content>
              <Switch.Control>
                <Switch.Thumb />
              </Switch.Control>
            </Switch.Content>
          </Switch>
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-ink">
            {t('backup.auto_interval')}
            <span className="ml-1 text-muted">{t('backup.auto_interval_suffix')}</span>
          </span>
          <Input
            key={config ? String(config.auto_backup_interval_days) : 'loading-interval'}
            type="number"
            variant="secondary"
            min={1}
            max={90}
            defaultValue={String(config?.auto_backup_interval_days ?? 7)}
            onBlur={e => updateAutoBackupSettings({ autoBackupIntervalDays: normalizeIntervalDays(e.target.value) })}
            className="w-[80px]"
          />
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-ink">{t('backup.auto_startup')}</span>
          <Switch
            isSelected={config?.auto_backup_on_startup ?? false}
            onChange={value => updateAutoBackupSettings({ autoBackupOnStartup: value })}
            size="sm"
          >
            <Switch.Content>
              <Switch.Control>
                <Switch.Thumb />
              </Switch.Control>
            </Switch.Content>
          </Switch>
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-ink">{t('backup.auto_change')}</span>
          <Switch
            isSelected={config?.auto_backup_on_change ?? false}
            onChange={value => updateAutoBackupSettings({ autoBackupOnChange: value })}
            size="sm"
          >
            <Switch.Content>
              <Switch.Control>
                <Switch.Thumb />
              </Switch.Control>
            </Switch.Content>
          </Switch>
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-ink">
            {t('backup.auto_retention')}
            <span className="ml-1 text-muted">{t('backup.auto_retention_suffix')}</span>
          </span>
          <Input
            key={config ? String(config.backup_retention_count) : 'loading-retention'}
            type="number"
            variant="secondary"
            min={1}
            max={50}
            defaultValue={String(config?.backup_retention_count ?? 10)}
            onBlur={e => updateAutoBackupSettings({ backupRetentionCount: normalizeRetentionCount(e.target.value) })}
            className="w-[80px]"
          />
        </div>
      </div>

      {dialogHolder}
    </div>
  )
}
