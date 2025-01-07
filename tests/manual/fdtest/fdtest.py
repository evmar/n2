"""
Test from https://github.com/evmar/n2/issues/14
Generates a build.ninja with a ton of parallel commands to see if we leak fds.
"""

import textwrap
def write(f):
    f.write(textwrap.dedent('''\
        rule b
            command = sleep 300; touch $out
        '''))
    for i in range(1000):
        f.write(f'build foo{i}: b\n')
    # n2 needs an explicit default target:
    f.write('default')
    for i in range(1000):
        f.write(f' foo{i}')
    f.write('\n')
with open('build.ninja', 'w', encoding='utf-8') as f:
    write(f)

