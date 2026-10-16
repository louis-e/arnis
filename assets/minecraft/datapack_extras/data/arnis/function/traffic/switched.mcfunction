# Every head in view took this switch; one out of view catches up when it comes back.
scoreboard players add #tl_epoch arnis 1
execute as @e[type=block_display,tag=arnis.tl] run scoreboard players operation @s arnis.tl = #tl_epoch arnis
