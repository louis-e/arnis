# Whether the executing player has open sky above them; leaves are no roof.
scoreboard players set #out arnis.ds 0
execute at @s positioned over motion_blocking_no_leaves positioned ~ ~-0.5 ~ if entity @s[dy=320] run scoreboard players set #out arnis.ds 1
