# Short of a railway crossing: while a train closes it (see xing/check), wait.
$execute unless score #x$(r) arnis.xing matches 1.. run return 0
$scoreboard players set @s arnis.wx $(r)
tag @s add arnis.w_rail
# Not for ever, should the crossing never be seen to open.
scoreboard players set @s arnis.ww 1200
