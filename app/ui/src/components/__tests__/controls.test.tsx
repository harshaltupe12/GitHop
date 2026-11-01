import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { expectNoA11yViolations } from '../../test/a11y';
import type { ProfileColor } from '../../lib/types';
import { ChipInput } from '../ChipInput';
import { ColorPicker } from '../ColorPicker';
import { Checkbox, Field, PasswordInput, TextInput } from '../Field';
import { Segmented } from '../Segmented';
import { Toggle } from '../Toggle';

describe('Field + TextInput', () => {
  it('label, helper and error are wired to the input', async () => {
    const { container } = render(
      <Field
        label="Email"
        helper="Tip: use your GitHub noreply email to keep it private."
        issues={[{ severity: 'error', message: 'Email already used by Office' }]}
      >
        {(p) => <TextInput {...p} mono defaultValue="harshal@company.com" />}
      </Field>,
    );
    const input = screen.getByRole('textbox', { name: 'Email' });
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription(/noreply email.*Email already used by Office/);
    expect(input.className).toContain('border-blocked-text');
    expect(input.className).toContain('font-mono');
    await expectNoA11yViolations(container);
  });

  it('warning and info issues do not mark the field invalid', () => {
    render(
      <Field label="Email" issues={[{ severity: 'warning', message: 'Consider a noreply email' }]}>
        {(p) => <TextInput {...p} />}
      </Field>,
    );
    const input = screen.getByRole('textbox', { name: 'Email' });
    expect(input).not.toHaveAttribute('aria-invalid');
    expect(screen.getByText('Consider a noreply email')).toBeInTheDocument();
  });

  it('disabled input', () => {
    render(<Field label="Label">{(p) => <TextInput {...p} disabled />}</Field>);
    expect(screen.getByRole('textbox', { name: 'Label' })).toBeDisabled();
  });
});

describe('PasswordInput', () => {
  it('toggles visibility', async () => {
    const { container } = render(
      <Field label="Password">
        {(p) => <PasswordInput {...p} defaultValue="secret-secret" />}
      </Field>,
    );
    const input = screen.getByLabelText('Password');
    expect(input).toHaveAttribute('type', 'password');
    await userEvent.click(screen.getByRole('button', { name: 'Show password' }));
    expect(input).toHaveAttribute('type', 'text');
    expect(screen.getByRole('button', { name: 'Hide password' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expectNoA11yViolations(container);
  });
});

describe('Checkbox', () => {
  it('is unchecked by default and toggles', async () => {
    const onChange = vi.fn();
    render(
      <Checkbox
        label="Also delete SSH keys created by GitHop"
        checked={false}
        onChange={onChange}
      />,
    );
    const cb = screen.getByRole('checkbox', { name: 'Also delete SSH keys created by GitHop' });
    expect(cb).not.toBeChecked();
    await userEvent.click(cb);
    expect(onChange).toHaveBeenCalledWith(true);
  });
});

function ChipHarness({ initial = [] as string[] }) {
  const [v, setV] = useState<string[]>(initial);
  return (
    <Field label="GitHub owners">
      {(p) => <ChipInput control={p} value={v} onChange={setV} />}
    </Field>
  );
}

describe('ChipInput', () => {
  it('Enter and comma add lower-cased chips; Backspace removes the last', async () => {
    const { container } = render(<ChipHarness />);
    const input = screen.getByRole('textbox', { name: 'GitHub owners' });
    await userEvent.type(input, 'Office-Org{Enter}');
    await userEvent.type(input, 'harshal,');
    expect(screen.getByText('office-org')).toBeInTheDocument();
    expect(screen.getByText('harshal')).toBeInTheDocument();
    await userEvent.type(input, '{Backspace}');
    expect(screen.queryByText('harshal')).toBeNull();
    await expectNoA11yViolations(container);
  });

  it('does not add duplicates; chip remove button works', async () => {
    render(<ChipHarness initial={['office-org']} />);
    const input = screen.getByRole('textbox', { name: 'GitHub owners' });
    await userEvent.type(input, 'office-org{Enter}');
    expect(screen.getAllByText('office-org')).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: 'Remove office-org' }));
    expect(screen.queryByText('office-org')).toBeNull();
  });
});

function ColorHarness({ taken = {} as Partial<Record<ProfileColor, string>> }) {
  const [c, setC] = useState<ProfileColor>('violet');
  return (
    <>
      <span id="lbl">Colour</span>
      <ColorPicker value={c} onChange={setC} takenBy={taken} labelledBy="lbl" />
    </>
  );
}

describe('ColorPicker', () => {
  it('8 swatches named by colour; selected one is checked', async () => {
    const { container } = render(<ColorHarness />);
    const group = screen.getByRole('radiogroup', { name: 'Colour' });
    expect(group.querySelectorAll('[role="radio"]')).toHaveLength(8);
    expect(screen.getByRole('radio', { name: 'violet' })).toHaveAttribute('aria-checked', 'true');
    await expectNoA11yViolations(container);
  });

  it('arrow keys move, skipping colours used by other profiles', async () => {
    render(<ColorHarness taken={{ pink: 'Freelance' }} />);
    screen.getByRole('radio', { name: 'violet' }).focus();
    await userEvent.keyboard('{ArrowRight}');
    expect(screen.getByRole('radio', { name: 'indigo' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('radio', { name: 'pink, used by freelance' })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
  });

  it('taken colour cannot be clicked', async () => {
    render(<ColorHarness taken={{ blue: 'Office' }} />);
    await userEvent.click(screen.getByRole('radio', { name: 'blue, used by office' }));
    expect(screen.getByRole('radio', { name: 'violet' })).toHaveAttribute('aria-checked', 'true');
  });
});

describe('Segmented', () => {
  it('radio semantics + arrow keys', async () => {
    const onChange = vi.fn();
    const { container } = render(
      <Segmented
        label="When identity is wrong"
        value="block"
        onChange={onChange}
        options={[
          { value: 'block', label: 'Block' },
          { value: 'warn', label: 'Warn' },
        ]}
      />,
    );
    const block = screen.getByRole('radio', { name: 'Block' });
    expect(block).toHaveAttribute('aria-checked', 'true');
    expect(block.className).toContain('bg-aqua');
    block.focus();
    await userEvent.keyboard('{ArrowRight}');
    expect(onChange).toHaveBeenCalledWith('warn');
    await userEvent.click(screen.getByRole('radio', { name: 'Warn' }));
    expect(onChange).toHaveBeenLastCalledWith('warn');
    await expectNoA11yViolations(container);
  });
});

describe('Toggle', () => {
  it('switch role with readable state', async () => {
    const onChange = vi.fn();
    const { container, rerender } = render(
      <>
        <span id="t">Activity log</span>
        <Toggle checked={false} onChange={onChange} labelledBy="t" />
      </>,
    );
    const sw = screen.getByRole('switch', { name: 'Activity log' });
    expect(sw).toHaveAttribute('aria-checked', 'false');
    await userEvent.click(sw);
    expect(onChange).toHaveBeenCalledWith(true);
    rerender(
      <>
        <span id="t">Activity log</span>
        <Toggle checked onChange={onChange} labelledBy="t" />
      </>,
    );
    expect(screen.getByRole('switch', { name: 'Activity log' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    await expectNoA11yViolations(container);
  });

  it('busy toggle ignores clicks but keeps focus', async () => {
    const onChange = vi.fn();
    render(<Toggle checked={false} onChange={onChange} label="Start with Windows" busy />);
    const sw = screen.getByRole('switch', { name: 'Start with Windows' });
    expect(sw).toHaveAttribute('aria-disabled', 'true');
    expect(sw).toBeEnabled();
    await userEvent.click(sw);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('disabled toggle', () => {
    render(<Toggle checked onChange={() => undefined} label="Activity log" disabled />);
    expect(screen.getByRole('switch', { name: 'Activity log' })).toBeDisabled();
  });
});
