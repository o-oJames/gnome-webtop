#!/usr/bin/env python3
"""
Minimal fake systemd-logind D-Bus service for container environments.
Provides just enough of the org.freedesktop.login1 API for GNOME Shell.
"""
import gi
gi.require_version('Gio', '2.0')
from gi.repository import Gio, GLib
import os
import sys
import struct

BUS_NAME = 'org.freedesktop.login1'
MANAGER_PATH = '/org/freedesktop/login1'
SESSION_PATH = '/org/freedesktop/login1/session/1'

# ── D-Bus introspection XML ──────────────────────────────────────────
MANAGER_XML = '''
<node>
  <interface name="org.freedesktop.login1.Manager">
    <method name="GetSession">
      <arg name="session_id" type="s" direction="in"/>
      <arg name="object_path" type="o" direction="out"/>
    </method>
    <method name="GetSessionByPID">
      <arg name="pid" type="u" direction="in"/>
      <arg name="object_path" type="o" direction="out"/>
    </method>
    <method name="ListSessions">
      <arg name="sessions" type="a(susso)" direction="out"/>
    </method>
    <method name="ListSessionsEx">
      <arg name="sessions" type="a(sussussso)" direction="out"/>
    </method>
    <method name="Inhibit">
      <arg name="what" type="s" direction="in"/>
      <arg name="who" type="s" direction="in"/>
      <arg name="why" type="s" direction="in"/>
      <arg name="mode" type="s" direction="in"/>
      <arg name="fd" type="h" direction="out"/>
    </method>
    <method name="CanSuspend">
      <arg name="result" type="s" direction="out"/>
    </method>
    <method name="CanHibernate">
      <arg name="result" type="s" direction="out"/>
    </method>
    <method name="CanPowerOff">
      <arg name="result" type="s" direction="out"/>
    </method>
    <method name="CanReboot">
      <arg name="result" type="s" direction="out"/>
    </method>
    <method name="Subscribe"/>
    <method name="Unsubscribe"/>
    <signal name="SessionNew">
      <arg name="session_id" type="s"/>
      <arg name="object_path" type="o"/>
    </signal>
    <signal name="SessionRemoved">
      <arg name="session_id" type="s"/>
      <arg name="object_path" type="o"/>
    </signal>
    <property name="Version" type="s" access="read"/>
    <property name="NAutoVTs" type="u" access="read"/>
    <property name="KillOnlyUsers" type="as" access="read"/>
    <property name="KillExcludeUsers" type="as" access="read"/>
    <property name="IdleHint" type="b" access="read"/>
  </interface>
</node>
'''

SESSION_XML = '''
<node>
  <interface name="org.freedesktop.login1.Session">
    <method name="Activate"/>
    <method name="Lock"/>
    <method name="Unlock"/>
    <method name="SetIdleHint">
      <arg name="idle" type="b" direction="in"/>
    </method>
    <method name="TakeControl">
      <arg name="force" type="b" direction="in"/>
    </method>
    <method name="ReleaseControl"/>
    <method name="SetType">
      <arg name="type" type="s" direction="in"/>
    </method>
    <method name="Kill">
      <arg name="who" type="s" direction="in"/>
      <arg name="signal" type="i" direction="in"/>
    </method>
    <method name="Terminate"/>
    <property name="Id" type="s" access="read"/>
    <property name="User" type="(uo)" access="read"/>
    <property name="Name" type="s" access="read"/>
    <property name="Timestamp" type="t" access="read"/>
    <property name="TimestampMonotonic" type="t" access="read"/>
    <property name="VTNr" type="u" access="read"/>
    <property name="Display" type="s" access="read"/>
    <property name="Remote" type="b" access="read"/>
    <property name="RemoteHost" type="s" access="read"/>
    <property name="RemoteUser" type="s" access="read"/>
    <property name="Service" type="s" access="read"/>
    <property name="Desktop" type="s" access="read"/>
    <property name="Scope" type="s" access="read"/>
    <property name="Leader" type="u" access="read"/>
    <property name="Audit" type="u" access="read"/>
    <property name="Type" type="s" access="read"/>
    <property name="Class" type="s" access="read"/>
    <property name="Active" type="b" access="read"/>
    <property name="State" type="s" access="read"/>
    <property name="IdleHint" type="b" access="read"/>
    <property name="CanGraphical" type="b" access="read"/>
    <property name="CanTTY" type="b" access="read"/>
  </interface>
</node>
'''

def get_uid():
    return os.getuid()

def on_manager_method_call(connection, sender, path, interface, method, params, invocation):
    if method == 'GetSession':
        invocation.return_value(GLib.Variant('(o)', (SESSION_PATH,)))
    elif method == 'GetSessionByPID':
        invocation.return_value(GLib.Variant('(o)', (SESSION_PATH,)))
    elif method == 'ListSessions':
        # a(susso): id, uid, name, seat_id, object_path
        sessions = [('1', get_uid(), os.environ.get('USER', 'user'), '', SESSION_PATH)]
        invocation.return_value(GLib.Variant('(a(susso))', (sessions,)))
    elif method == 'ListSessionsEx':
        # a(sussussso)
        sessions = [('1', get_uid(), os.environ.get('USER', 'user'), '', 'active', '', '', SESSION_PATH)]
        invocation.return_value(GLib.Variant('(a(sussussso))', (sessions,)))
    elif method == 'Inhibit':
        # Return a pipe fd (GNOME Shell just needs a valid fd)
        r, w = os.pipe()
        fd_list = Gio.UnixFDList.new_from_array([r])
        invocation.return_value_with_unix_fd_list(GLib.Variant('(h)', (0,)), fd_list)
        os.close(r)
        os.close(w)
    elif method in ('CanSuspend', 'CanHibernate', 'CanPowerOff', 'CanReboot'):
        invocation.return_value(GLib.Variant('(s)', ('na',)))
    elif method in ('Subscribe', 'Unsubscribe'):
        invocation.return_value(None)
    else:
        invocation.return_error(Gio.DBusError.quark(), Gio.DBusError.UNKNOWN_METHOD,
                                f'Unknown method: {method}')

def on_manager_get_property(connection, sender, path, interface, prop):
    props = {
        'Version': GLib.Variant('s', '255.4-1ubuntu8.6 (fake)'),
        'NAutoVTs': GLib.Variant('u', 0),
        'KillOnlyUsers': GLib.Variant('as', []),
        'KillExcludeUsers': GLib.Variant('as', ['root']),
        'IdleHint': GLib.Variant('b', False),
    }
    return props.get(prop)

def on_session_method_call(connection, sender, path, interface, method, params, invocation):
    # All session methods are no-ops
    invocation.return_value(None)

def on_session_get_property(connection, sender, path, interface, prop):
    uid = get_uid()
    username = os.environ.get('USER', 'user')
    display = os.environ.get('DISPLAY', ':1')
    props = {
        'Id': GLib.Variant('s', '1'),
        'User': GLib.Variant('(uo)', (uid, f'/org/freedesktop/login1/user/_{uid}')),
        'Name': GLib.Variant('s', username),
        'Timestamp': GLib.Variant('t', 0),
        'TimestampMonotonic': GLib.Variant('t', 0),
        'VTNr': GLib.Variant('u', 0),
        'Display': GLib.Variant('s', display),
        'Remote': GLib.Variant('b', False),
        'RemoteHost': GLib.Variant('s', ''),
        'RemoteUser': GLib.Variant('s', ''),
        'Service': GLib.Variant('s', 'fake-logind'),
        'Desktop': GLib.Variant('s', 'GNOME'),
        'Scope': GLib.Variant('s', 'fake'),
        'Leader': GLib.Variant('u', os.getpid()),
        'Audit': GLib.Variant('u', 0),
        'Type': GLib.Variant('s', 'x11'),
        'Class': GLib.Variant('s', 'user'),
        'Active': GLib.Variant('b', True),
        'State': GLib.Variant('s', 'active'),
        'IdleHint': GLib.Variant('b', False),
        'CanGraphical': GLib.Variant('b', True),
        'CanTTY': GLib.Variant('b', False),
    }
    return props.get(prop)

def main():
    loop = GLib.MainLoop()
    connection = Gio.bus_get_sync(Gio.BusType.SYSTEM, None)

    # Register Manager
    manager_info = Gio.DBusNodeInfo.new_for_xml(MANAGER_XML).interfaces[0]
    connection.register_object(
        MANAGER_PATH, manager_info,
        on_manager_method_call, on_manager_get_property, None
    )

    # Register Session
    session_info = Gio.DBusNodeInfo.new_for_xml(SESSION_XML).interfaces[0]
    connection.register_object(
        SESSION_PATH, session_info,
        on_session_method_call, on_session_get_property, None
    )

    # Own the bus name
    Gio.bus_own_name_on_connection(
        connection, BUS_NAME,
        Gio.BusNameOwnerFlags.NONE, None, None
    )

    print('✅ fake-logind: org.freedesktop.login1 registered on system bus', flush=True)
    loop.run()

if __name__ == '__main__':
    main()
