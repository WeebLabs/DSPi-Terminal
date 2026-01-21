#!/usr/bin/env python3
"""
DSPi CLI - Command Line Interface for DSPi Audio Processor
"""

import os
import sys
import time
import math
import struct
import threading
import json
import logging
from pathlib import Path
from typing import List, Optional, Tuple, Dict, Union

# Third-party imports
try:
    import usb.core
    import usb.util
    from prompt_toolkit import PromptSession, Application
    from prompt_toolkit.completion import WordCompleter, NestedCompleter
    from prompt_toolkit.shortcuts import yes_no_dialog
    from prompt_toolkit.layout.containers import Window, HSplit, VSplit
    from prompt_toolkit.layout.controls import BufferControl, FormattedTextControl
    from prompt_toolkit.layout.layout import Layout
    from prompt_toolkit.formatted_text import FormattedText, HTML
    from prompt_toolkit.styles import Style
    from prompt_toolkit.key_binding import KeyBindings
    from prompt_toolkit.buffer import Buffer
    from prompt_toolkit.document import Document
    from prompt_toolkit.application import get_app
    from rich.console import Console
    from rich.panel import Panel
    from rich.table import Table
    from rich import box
    from rich.align import Align
    from rich.text import Text
except ImportError:
    print("Missing dependencies. Please run:")
    print("pip install pyusb prompt_toolkit rich")
    sys.exit(1)

# --- Configuration & Constants ---

VENDOR_ID = 0x2e8a
PRODUCT_ID = 0xfeaa

# Request Codes
REQ_SET_EQ_PARAM    = 0x42
REQ_GET_EQ_PARAM    = 0x43
REQ_SET_PREAMP      = 0x44
REQ_GET_PREAMP      = 0x45
REQ_SET_BYPASS      = 0x46
REQ_GET_BYPASS      = 0x47
REQ_SET_DELAY       = 0x48
REQ_GET_DELAY       = 0x49
REQ_GET_STATUS      = 0x50
REQ_SAVE_PARAMS     = 0x51
REQ_LOAD_PARAMS     = 0x52
REQ_FACTORY_RESET   = 0x53

# Interface constants
INTERFACE_VENDOR = 0

# Colors
COLOR_AMBER = "#ff9900"
COLOR_CYAN = "#00ffff"
COLOR_CYAN_DIM = "#008888"
COLOR_RED = "#ff5555"

# --- Logger Setup ---
logging.basicConfig(level=logging.ERROR, filename="dspi_cli.log")
logger = logging.getLogger("dspi")

# --- Data Models ---

class FilterType:
    FLAT = 0
    PEAKING = 1
    LOW_SHELF = 2
    HIGH_SHELF = 3
    LOW_PASS = 4
    HIGH_PASS = 5

    @classmethod
    def from_str(cls, s: str) -> int:
        s = s.lower()
        if s in ('flat', 'off'): return cls.FLAT
        if s in ('peak', 'peaking', 'pk', 'peq'): return cls.PEAKING
        if s in ('ls', 'lowshelf', 'low_shelf'): return cls.LOW_SHELF
        if s in ('hs', 'highshelf', 'high_shelf'): return cls.HIGH_SHELF
        if s in ('lp', 'lowpass', 'low_pass'): return cls.LOW_PASS
        if s in ('hp', 'highpass', 'high_pass'): return cls.HIGH_PASS
        return cls.FLAT

    @classmethod
    def to_str(cls, v: int) -> str:
        if v == cls.FLAT: return "Flat"
        if v == cls.PEAKING: return "Peak"
        if v == cls.LOW_SHELF: return "LowShelf"
        if v == cls.HIGH_SHELF: return "HighShelf"
        if v == cls.LOW_PASS: return "LowPass"
        if v == cls.HIGH_PASS: return "HighPass"
        return "Unknown"

class Channel:
    MASTER_LEFT = 0
    MASTER_RIGHT = 1
    OUT_LEFT = 2
    OUT_RIGHT = 3
    OUT_SUB = 4
    
    NAMES = {0: "Master L", 1: "Master R", 2: "Out L", 3: "Out R", 4: "Sub"}
    BAND_COUNTS = {0: 10, 1: 10, 2: 2, 3: 2, 4: 2}

class FilterParams:
    def __init__(self, type_id=0, freq=1000.0, q=0.707, gain=0.0):
        self.type = type_id
        self.freq = freq
        self.q = q
        self.gain = gain

    def __repr__(self):
        return f"{FilterType.to_str(self.type)} {self.freq:.1f}Hz Q{self.q:.2f} {self.gain:+.1f}dB"

# --- DSP Math ---

class DSPMath:
    SAMPLE_RATE = 48000.0

    @staticmethod
    def calculate_coefficients(p: FilterParams):
        if p.type == FilterType.FLAT:
            return 1.0, 0.0, 0.0, 0.0, 0.0 # b0..b2, a1..a2

        w = 2.0 * math.pi * p.freq / DSPMath.SAMPLE_RATE
        sn = math.sin(w)
        cs = math.cos(w)
        alpha = sn / (2.0 * p.q)
        A = 10.0 ** (p.gain / 40.0)

        b0 = b1 = b2 = a0 = a1 = a2 = 0.0

        if p.type == FilterType.LOW_PASS:
            b0 = (1 - cs)/2; b1 = 1 - cs; b2 = (1 - cs)/2
            a0 = 1 + alpha; a1 = -2 * cs; a2 = 1 - alpha
        elif p.type == FilterType.HIGH_PASS:
            b0 = (1 + cs)/2; b1 = -(1 + cs); b2 = (1 + cs)/2
            a0 = 1 + alpha; a1 = -2 * cs; a2 = 1 - alpha
        elif p.type == FilterType.PEAKING:
            b0 = 1 + alpha * A; b1 = -2 * cs; b2 = 1 - alpha * A
            a0 = 1 + alpha / A; a1 = -2 * cs; a2 = 1 - alpha / A
        elif p.type == FilterType.LOW_SHELF:
            sqA = math.sqrt(A)
            b0 = A*((A+1)-(A-1)*cs+2*sqA*alpha)
            b1 = 2*A*((A-1)-(A+1)*cs)
            b2 = A*((A+1)-(A-1)*cs-2*sqA*alpha)
            a0 = (A+1)+(A-1)*cs+2*sqA*alpha
            a1 = -2*((A-1)+(A+1)*cs)
            a2 = (A+1)+(A-1)*cs-2*sqA*alpha
        elif p.type == FilterType.HIGH_SHELF:
            sqA = math.sqrt(A)
            b0 = A*((A+1)+(A-1)*cs+2*sqA*alpha)
            b1 = -2*A*((A-1)+(A+1)*cs)
            b2 = A*((A+1)+(A-1)*cs-2*sqA*alpha)
            a0 = (A+1)-(A-1)*cs+2*sqA*alpha
            a1 = 2*((A-1)-(A+1)*cs)
            a2 = (A+1)-(A-1)*cs-2*sqA*alpha

        return b0/a0, b1/a0, b2/a0, a1/a0, a2/a0

    @staticmethod
    def response_at(freq: float, filters: List[FilterParams]) -> float:
        mag_sq_total = 1.0
        w = 2.0 * math.pi * freq / DSPMath.SAMPLE_RATE
        cos_w = math.cos(w)
        cos_2w = math.cos(2.0 * w)
        sin_w = math.sin(w)
        sin_2w = math.sin(2.0 * w)

        for f in filters:
            if f.type == FilterType.FLAT: continue
            
            b0, b1, b2, a1, a2 = DSPMath.calculate_coefficients(f)
            
            num_r = b0 + b1 * cos_w + b2 * cos_2w
            num_i = -(b1 * sin_w + b2 * sin_2w)
            den_r = 1.0 + a1 * cos_w + a2 * cos_2w
            den_i = -(a1 * sin_w + a2 * sin_2w)
            
            num_mag_sq = num_r*num_r + num_i*num_i
            den_mag_sq = den_r*den_r + den_i*den_i
            
            if den_mag_sq > 1e-15:
                mag_sq_total *= (num_mag_sq / den_mag_sq)
                
        return 10.0 * math.log10(mag_sq_total) if mag_sq_total > 0 else -100.0

# --- USB Handler ---

class DeviceHandler:
    def __init__(self):
        self.dev = None
        self.connected = False
        self.lock = threading.Lock()
        
        # State Cache
        self.filters: Dict[int, List[FilterParams]] = {
            c: [FilterParams() for _ in range(Channel.BAND_COUNTS[c])] 
            for c in Channel.BAND_COUNTS
        }
        self.delays: Dict[int, float] = {c: 0.0 for c in Channel.BAND_COUNTS}
        self.preamp = 0.0
        self.bypass = False
        self.status = {'peaks': [0.0]*5, 'cpu': [0, 0]}
        
    def connect(self):
        try:
            self.dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
            if self.dev is None:
                self.connected = False
                return False
                
            # On macOS, Interface 0/1 are owned by Audio Driver. 
            # Interface 2 is Vendor and should be free.
            # We must claim it to prevent the OS from interfering (though unlikely for Vendor class)
            # or just use it.
            
            cfg = self.dev.get_active_configuration()
            intf = cfg[(INTERFACE_VENDOR, 0)]
            
            # Detach kernel driver if necessary (Linux mostly)
            if self.dev.is_kernel_driver_active(INTERFACE_VENDOR):
                try:
                    self.dev.detach_kernel_driver(INTERFACE_VENDOR)
                except usb.core.USBError as e:
                    logger.error(f"Could not detach kernel driver: {e}")

            self.connected = True
            self.refresh_all()
            return True
        except Exception as e:
            logger.error(f"Connection error: {e}")
            self.connected = False
            return False

    def send_control(self, req, val, idx, data=None):
        if not self.connected: return
        with self.lock:
            try:
                bmRequestType = 0x41 # Host to Device | Vendor | Interface
                self.dev.ctrl_transfer(bmRequestType, req, val, idx, data)
            except Exception as e:
                logger.error(f"Send control error: {e}")
                self.connected = False

    def get_control(self, req, val, idx, length):
        if not self.connected: return None
        with self.lock:
            try:
                bmRequestType = 0xC1 # Device to Host | Vendor | Interface
                ret = self.dev.ctrl_transfer(bmRequestType, req, val, idx, length)
                return ret
            except Exception as e:
                logger.error(f"Get control error: {e}")
                self.connected = False
                return None

    def refresh_all(self):
        self.get_preamp()
        self.get_bypass()
        for ch in self.filters:
            for b in range(len(self.filters[ch])):
                self.get_filter(ch, b)
            if ch >= 2: # Output channels
                self.get_delay(ch)

    # --- Commands ---

    def set_filter(self, ch, band, p: FilterParams):
        self.filters[ch][band] = p
        # Pack: ch(1), band(1), type(1), res(1), freq(4f), q(4f), gain(4f)
        data = struct.pack('<BBBBfff', ch, band, p.type, 0, p.freq, p.q, p.gain)
        self.send_control(REQ_SET_EQ_PARAM, 0, 0, data)

    def get_filter(self, ch, band):
        # wValue = (ch << 8) | (band << 4) | param
        # We need 4 calls to get all fields? No, the code in firmware 
        # REQ_GET_EQ_PARAM returns 4 bytes based on param index (0..3)
        # 0=Type, 1=Freq, 2=Q, 3=Gain
        
        def get_val(param, fmt):
            wVal = (ch << 8) | (band << 4) | param
            ret = self.get_control(REQ_GET_EQ_PARAM, wVal, 0, 4)
            if ret: return struct.unpack(fmt, ret)[0]
            return 0
            
        t_raw = get_val(0, '<I')
        freq = get_val(1, '<f')
        q = get_val(2, '<f')
        gain = get_val(3, '<f')
        
        self.filters[ch][band] = FilterParams(t_raw, freq, q, gain)

    def set_preamp(self, db: float):
        self.preamp = db
        data = struct.pack('<f', db)
        self.send_control(REQ_SET_PREAMP, 0, 0, data)

    def get_preamp(self):
        ret = self.get_control(REQ_GET_PREAMP, 0, 0, 4)
        if ret:
            self.preamp = struct.unpack('<f', ret)[0]

    def set_bypass(self, enabled: bool):
        self.bypass = enabled
        data = struct.pack('<B', 1 if enabled else 0)
        self.send_control(REQ_SET_BYPASS, 0, 0, data)

    def get_bypass(self):
        ret = self.get_control(REQ_GET_BYPASS, 0, 0, 1)
        if ret:
            self.bypass = (ret[0] != 0)

    def set_delay(self, ch, ms: float):
        self.delays[ch] = ms
        data = struct.pack('<f', ms)
        self.send_control(REQ_SET_DELAY, ch, 0, data)
        
    def get_delay(self, ch):
        ret = self.get_control(REQ_GET_DELAY, ch, 0, 4)
        if ret:
            self.delays[ch] = struct.unpack('<f', ret)[0]

    def update_status(self):
        # REQ_GET_STATUS wValue=9 returns 12 bytes
        ret = self.get_control(REQ_GET_STATUS, 9, 0, 12)
        if ret:
            # peaks (5 * uint16), cpu0(u8), cpu1(u8)
            peaks = list(struct.unpack('<HHHHH', ret[:10]))
            self.status['peaks'] = [p/65535.0 for p in peaks]
            self.status['cpu'][0] = ret[10]
            self.status['cpu'][1] = ret[11]

    def save(self):
        ret = self.get_control(REQ_SAVE_PARAMS, 0, 0, 1)
        return ret[0] if ret else 0

    def load(self):
        ret = self.get_control(REQ_LOAD_PARAMS, 0, 0, 1)
        if ret and ret[0] == 0: # FLASH_OK
            self.refresh_all()
            return True
        return False
        
    def factory_reset(self):
        ret = self.get_control(REQ_FACTORY_RESET, 0, 0, 1)
        if ret and ret[0] == 0:
            self.refresh_all()
            return True
        return False

# --- AutoEQ ---

class AutoEQ:
    def __init__(self, path: str):
        self.entries = []
        try:
            with open(path, 'r') as f:
                data = json.load(f)
                self.entries = data.get('entries', [])
        except Exception:
            pass
            
    def search(self, query: str):
        q = query.lower()
        return [e for e in self.entries if q in e['id'].lower() or q in e['model'].lower()]

    def get_by_id(self, entry_id: str):
        for e in self.entries:
            if e['id'] == entry_id: return e
        return None

# --- UI Components ---

def make_meter(val: float, width=20):
    # Logarithmic meter
    if val < 0.0001: db = -60
    else: db = 20 * math.log10(val)
    
    # Scale -60dB to 0dB -> 0 to 1.0
    norm = (db + 60.0) / 60.0
    if norm < 0: norm = 0
    if norm > 1: norm = 1
    
    filled = int(norm * width)
    bar = "█" * filled + "░" * (width - filled)
    
    col = "green"
    if norm > 0.8: col = "yellow"
    if norm > 0.95: col = "red"
    
    # Return string only, let get_status_bar wrap it
    return f"<style color='{col}'>{bar}</style> <style color='#888888'>{db:5.1f}dB</style>"

def render_graph(handler: DeviceHandler, channel: int, height=10, width=60):
    filters = handler.filters.get(channel, [])
    if not filters: return ""

    # Frequencies (Log scale)
    freqs = [20 * (1000**(i/(width-1))) for i in range(width)]
    
    # Calculate response
    points = []
    min_db, max_db = -20.0, 20.0
    
    for f in freqs:
        db = DSPMath.response_at(f, filters)
        # Add preamp/bypass logic if needed, but graph usually shows filter curve
        points.append(db)

    # Render
    lines = []
    
    # Grid & Plot
    for r in range(height):
        row_db = max_db - (r / (height-1)) * (max_db - min_db)
        line_str = ""
        for c in range(width):
            db = points[c]
            # Simple threshold rendering
            if abs(db - row_db) < ((max_db-min_db)/height)/2.0:
                line_str += "●"
            elif abs(row_db) < 0.1: # Zero line
                line_str += "─" 
            else:
                line_str += " "
        
        # Y-Axis Label
        label = f"{row_db:+3.0f}┤"
        if r == 0: label = f"{max_db:+3.0f}┬"
        if r == height-1: label = f"{min_db:+3.0f}┴"
        
        lines.append(f"<style color='{COLOR_AMBER}'>{label}</style><style color='{COLOR_CYAN}'>{line_str}</style>")
    
    # X-Axis
    x_axis = "    └" + "─" * width 
    lines.append(f"<style color='{COLOR_AMBER}'>{x_axis}</style>")
    
    return "\n".join(lines)

# --- Main Application ---

handler = DeviceHandler()
autoeq = AutoEQ("DSPi Console/AutoEQ/autoeq_database.json")

# Shared state for TUI
graph_channel = Channel.MASTER_LEFT
show_graph = True

def get_status_bar():
    # Fetch status if connected
    if not handler.connected:
        return HTML(f"<style bg='{COLOR_RED}' fg='white'> DISCONNECTED </style> Waiting for device...")
    
    s = handler.status
    
    peaks = s['peaks']
    cpu = s['cpu']
    
    # Format
    # CPU: [==  ] 12% | ML: [====] -10dB | MR: ...
    
    parts = []
    parts.append(f"<style color='{COLOR_AMBER}'>CPU0:</style> {cpu[0]:2d}%")
    parts.append(f"<style color='{COLOR_AMBER}'>CPU1:</style> {cpu[1]:2d}%")
    parts.append(" │ ")
    parts.append(f"<style color='{COLOR_CYAN}'>L:</style> ")
    parts.append(make_meter(peaks[0], 10))
    parts.append(f" <style color='{COLOR_CYAN}'>R:</style> ")
    parts.append(make_meter(peaks[1], 10))
    parts.append(" │ ")
    parts.append(f"<style color='{COLOR_CYAN}'>Sub:</style> ")
    parts.append(make_meter(peaks[4], 10))
    
    if handler.bypass:
        parts.append(f" <style bg='{COLOR_RED}' fg='white'> BYPASS </style>")
    
    return HTML("".join(parts))

def get_graph_text():
    if not show_graph: return ""
    ch_name = Channel.NAMES.get(graph_channel, "Unknown")
    g_content = render_graph(handler, graph_channel)
    return HTML(f"<style color='{COLOR_AMBER}'>Frequency Response ({ch_name})</style>\n{g_content}")

def monitor_loop(app: Application):
    while True:
        if not handler.connected:
            handler.connect()
        else:
            handler.update_status()
        
        app.invalidate()
        time.sleep(0.1)

# --- REPL Logic ---


# --- Completer ---

base_commands = {
    'filter': { str(i): { str(b): {'peak': None, 'lowshelf': None, 'highshelf': None, 'lowpass': None, 'highpass': None, 'flat': None} for b in range(10) } for i in range(5) },
    'preamp': None,
    'delay': { str(i): None for i in range(5) },
    'bypass': {'on', 'off'},
    'save': None, 'load': None, 'reset': None,
    'graph': { str(i): None for i in range(5) },
    'autoeq': {'search': None, 'apply': None},
    'exit': None, 'help': None
}

# Create a completer dict that includes both "command" and "/command"
completer_dict = base_commands.copy()
for cmd, sub in base_commands.items():
    completer_dict[f"/{cmd}"] = sub

command_completer = NestedCompleter.from_nested_dict(completer_dict)

def handle_command(text: str, print_func):
    parts = text.strip().split()
    if not parts: return

    cmd = parts[0].lower()
    if cmd.startswith('/'):
        cmd = cmd[1:]

    try:
        if cmd == 'exit':
            sys.exit(0)
            
        elif cmd == 'help':
            print_func(HTML(f"""
<style color='{COLOR_CYAN}'>Available Commands:</style>
  <style color='{COLOR_AMBER}'>filter</style> <ch> <band> <type> <freq> <q> <gain>  Set filter parameters
  <style color='{COLOR_AMBER}'>preamp</style> <db>                               Set global preamp gain
  <style color='{COLOR_AMBER}'>delay</style> <ch> <ms>                             Set channel delay
  <style color='{COLOR_AMBER}'>bypass</style> <on|off>                            Toggle global bypass
  <style color='{COLOR_AMBER}'>save</style>                                      Save to flash
  <style color='{COLOR_AMBER}'>load</style>                                      Load from flash
  <style color='{COLOR_AMBER}'>reset</style>                                     Factory reset
  <style color='{COLOR_AMBER}'>graph</style> <ch>                                  Show graph for channel
  <style color='{COLOR_AMBER}'>autoeq search</style> <query>                       Search AutoEQ database
  <style color='{COLOR_AMBER}'>autoeq apply</style> <id>                           Apply AutoEQ profile
            """))

        elif cmd == 'filter':
            # filter 0 0 peak 1000 0.7 5.0
            if len(parts) < 7:
                print_func("Usage: filter <ch> <band> <type> <freq> <q> <gain>")
                return
            ch = int(parts[1])
            band = int(parts[2])
            ftype = FilterType.from_str(parts[3])
            freq = float(parts[4])
            q = float(parts[5])
            gain = float(parts[6])
            
            p = FilterParams(ftype, freq, q, gain)
            handler.set_filter(ch, band, p)
            print_func(f"Set Ch{ch} Band{band}: {p}")

        elif cmd == 'preamp':
            db = float(parts[1])
            handler.set_preamp(db)
            print_func(f"Preamp: {db} dB")

        elif cmd == 'bypass':
            state = parts[1].lower() == 'on'
            handler.set_bypass(state)
            print_func(f"Bypass: {'ON' if state else 'OFF'}")

        elif cmd == 'delay':
            ch = int(parts[1])
            ms = float(parts[2])
            handler.set_delay(ch, ms)
            print_func(f"Delay Ch{ch}: {ms} ms")

        elif cmd == 'save':
            if handler.save() == 0: print_func("Saved to Flash.")
            else: print_func("Error saving.")

        elif cmd == 'load':
            if handler.load(): print_func("Loaded from Flash.")
            else: print_func("Error loading.")

        elif cmd == 'reset':
            if handler.factory_reset(): print_func("Factory Reset Complete.")
            else: print_func("Error resetting.")

        elif cmd == 'graph':
            global graph_channel, show_graph
            if len(parts) > 1:
                graph_channel = int(parts[1])
            show_graph = True
            print_func(f"Showing graph for {Channel.NAMES.get(graph_channel)}")

        elif cmd == 'autoeq':
            sub = parts[1].lower()
            if sub == 'search':
                query = " ".join(parts[2:])
                res = autoeq.search(query)
                print_func(HTML(f"<style color='{COLOR_CYAN}'>Found {len(res)} results:</style>"))
                for r in res[:10]: # Limit 10
                    print_func(f"  {r['id']}")
                if len(res) > 10: print_func("  ...")
            
            elif sub == 'apply':
                eid = parts[2]
                entry = autoeq.get_by_id(eid)
                if not entry:
                    print_func("Profile not found.")
                    return
                
                print_func(f"Applying {entry['model']}...")
                
                # Apply Preamp
                handler.set_preamp(entry['preamp'])
                
                # Apply Filters to Master L/R
                for ch in [Channel.MASTER_LEFT, Channel.MASTER_RIGHT]:
                    for i, f in enumerate(entry['filters']):
                        if i >= 10: break
                        p = FilterParams(
                            FilterType.from_str(f['type']),
                            f['freq'],
                            f['q'],
                            f['gain']
                        )
                        handler.set_filter(ch, i, p)
                    
                    # Clear remaining
                    for i in range(len(entry['filters']), 10):
                        handler.set_filter(ch, i, FilterParams())
                        
                print_func("Profile applied.")

        else:
            print_func(f"Unknown command: {cmd}")

    except Exception as e:
        print_func(f"Error: {e}")

def main():
    # Setup prompt
    session = PromptSession(completer=command_completer)
    
    def bottom_toolbar():
        return get_status_bar()

    # Console for rich printing
    console = Console()
    console.clear()

    title_art = """
██████╗ ███████╗██████╗ ██╗     ██████╗ ██████╗ ███╗   ██╗███████╗ ██████╗ ██╗     ███████╗
██╔══██╗██╔════╝██╔══██╗██║    ██╔════╝██╔═══██╗████╗  ██║██╔════╝██╔═══██╗██║     ██╔════╝
██║  ██║███████╗██████╔╝██║    ██║     ██║   ██║██╔██╗ ██║███████╗██║   ██║██║     █████╗  
██║  ██║╚════██║██╔═══╝ ██║    ██║     ██║   ██║██║╚██╗██║╚════██║██║   ██║██║     ██╔══╝  
██████╔╝███████║██║     ██║    ╚██████╗╚██████╔╝██║ ╚████║███████║╚██████╔╝███████╗███████╗
╚═════╝ ╚══════╝╚═╝     ╚═╝     ╚═════╝ ╚═════╝ ╚═╝  ╚═══╝╚══════╝ ╚═════╝ ╚══════╝╚══════╝
    """
    console.print(Align.center(Text(title_art, style=COLOR_AMBER)))
    
    print(f"DSPi CLI v1.0")
    print(f"Connecting...")
    
    # Initial connection attempt
    if handler.connect():
        console.print(f"[green]Connected Successfully[/green] on Interface {INTERFACE_VENDOR} (VID: {hex(VENDOR_ID)}, PID: {hex(PRODUCT_ID)})")
    else:
        console.print(f"[red]Connection Failed.[/red] Device not found or busy.")
    
    app_ref = [None]
    
    def monitor_wrapper():
        while True:
            if not handler.connected:
                handler.connect()
            else:
                handler.update_status()
            
            if app_ref[0]:
                app_ref[0].invalidate()
            
            time.sleep(0.1)

    t = threading.Thread(target=monitor_wrapper, daemon=True)
    t.start()

    # Main Loop
    while True:
        try:
            text = session.prompt(
                HTML(f"<style color='{COLOR_CYAN}'>dspi> </style>"), 
                bottom_toolbar=bottom_toolbar,
                refresh_interval=0.1
            )
            
            # Capture app reference once
            if app_ref[0] is None:
                app_ref[0] = session.app

            handle_command(text, console.print)
            
            if show_graph and text.strip() == 'status':
                 console.print(render_graph(handler, graph_channel))

        except KeyboardInterrupt:
            break
        except EOFError:
            break

if __name__ == "__main__":
    main()
