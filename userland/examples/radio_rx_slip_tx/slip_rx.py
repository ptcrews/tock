#!/usr/bin/python
# Receive bytes from serial port, save to a file.
# Uses serial line IP (SLIP) as specified in RFC 1055.

import datetime
import os
import serial
import sys

addr      = '/dev/ttyUSB0'                   # serial port to read data from
baud      = 128000                           # baud rate for serial port
date      = datetime.datetime.now()
directory = 'logs'
fname     = 'log_' + str(date) + '.txt'      # log file to save data in
fmode     = 'w'                              # log file mode = APPEND

# SLIP special character codes
END     = 0300    # indicates end of packet
ESC     = 0333    # indicates byte stuffing
ESC_END = 0334    # ESC ESC_END means END data byte
ESC_ESC = 0335    # ESC ESC_ESC means ESC data byte

max_packet_len = 100

def recv_packet():
    packet = []
    received = 0
    while (1):
        c = ser.read()

        # if it's an END character then we're done with
        # the packet
        if c == END:
            # a minor optimization: if there is no
            # data in the packet, ignore it. This is
            # meant to avoid bothering IP with all
            # the empty packets generated by the
            # duplicate END characters which are in
            # turn sent to try to detect line noise.
            if (received):
                return ''.join(packet)
            else:
                break

        # if it's the same code as an ESC character, wait
        # and get another character and then figure out
        # what to store in the packet based on that.
        elif c == ESC:
            c = ser.read()

            # if "c" is not one of these two, then we
            # have a protocol violation.  The best bet
            # seems to be to leave the byte alone and
            # just stuff it into the packet
            if c == ESC_END:
                c = END
                break
            elif c == ESC_ESC:
                c = ESC
                break

        # here we fall into the default handler and let
        # it store the character for us
        else:
            if received < max_packet_len:
                packet.append(c)

if not os.path.exists(directory):
    os.makedirs(directory)

with serial.Serial(addr, baud) as ser, open(directory + '/' + fname, fmode) as f:
    while (1):
        print "LOOP"
        packet = recv_packet()
        sys.stdout.write(packet)    # echo packet on-screen as ASCII
        sys.stdout.flush()          # make sure it actually gets written out
        f.write(packet)             # write line of text to file
        f.flush()                   # make sure it actually gets written out