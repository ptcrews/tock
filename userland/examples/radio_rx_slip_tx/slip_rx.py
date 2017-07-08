#!/usr/bin/python
# Receive bytes from serial port, save to a file.
# Uses serial line IP (SLIP) as specified in RFC 1055.

import datetime
import os
import serial
import sys

addr      = '/dev/ttyUSB0'                         # serial port to read data from
baud      = 128000                                 # baud rate for serial port
log_dir = 'logs'
fname     = 'log_' + str(datetime.datetime.now()) + '.txt'  # log file to save data in
fmode     = 'w'                                    # log file mode = APPEND
packet_dir = 'packets'

# SLIP special character codes, written in octal
END     = 0300  # 0300 indicates end of packet
ESC     = 0333  # 0333 indicates byte stuffing
ESC_END = 0334  # 0334 ESC ESC_END means END data byte
ESC_ESC = 0335  # 0335 ESC ESC_ESC means ESC data byte

print 'START SLIP RECEIVE'
print 'END:',     END
print 'ESC:',     ESC
print 'ESC_END:', ESC_END
print 'ESC_ESC:', ESC_ESC

max_packet_len = 100

# Requires wireshark (text2pcap) to be installed.
def str_to_pcap_file(packet_str, outfile):
    cmd = 'echo 0000    ' + packet_str + ' >> tmp.txt'
    os.system(cmd)
    cmd = 'text2pcap ' + 'tmp.txt ' + outfile 
    os.system(cmd)
    os.system('rm tmp.txt')

for directory in [log_dir, packet_dir]:
    if not os.path.exists(directory):
        os.makedirs(directory)

with serial.Serial(addr, baud) as ser, open(log_dir + '/' + fname, fmode) as f, open(log_dir + '/log.txt', fmode) as f_cur:
    while (1):

        packet = []
        received = 0
        while (1):
            c = ord(ser.read())  # read single byte, output is str

            if received == 0:
                print '\nRECEIVING PACKET:'

            print chr(c),
            # sys.stdout.write(c)
            # sys.stdout.flush()

            # if it's an END character then we're done with
            # the packet
            if c == END:
                # a minor optimization: if there is no
                # data in the packet, ignore it. This is
                # meant to avoid bothering IP with all
                # the empty packets generated by the
                # duplicate END characters which are in
                # turn sent to try to detect line noise.
                if received > 0:
                    break
                else:
                    continue

            # if it's the same code as an ESC character, wait
            # and get another character and then figure out
            # what to store in the packet based on that.
            elif c == ESC:
                c = ser.read()
                # if 'c' is not one of these two, then we
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
                    packet.append(chr(c))
                    received += 1

        packet_str = ''.join(packet)

        print '\nDECODED PACKET (LENGTH ' + str(received) + '):'
        print packet_str

        # sys.stdout.write(packet)    # echo packet on-screen as ASCII
        # sys.stdout.flush()          # make sure it actually gets written out
        for file in [f, f_cur]:
            file.write(packet_str)        # write line of text to file
            file.flush()              # make sure it actually gets written out

        packet = ' '.join(map(lambda c: c.encode('hex'), packet))

        print '\nHEX ENCODED PACKET:'
        print packet

        pkt_fname = packet_dir + '/pkt_' + str(datetime.datetime.now()) + '.pcap'
        str_to_pcap_file(packet, pkt_fname)

        break