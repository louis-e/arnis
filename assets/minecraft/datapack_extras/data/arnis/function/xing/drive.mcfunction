scoreboard players set #xt arnis.xing 0
# A mast or boom that was out of view when its crossing changed catches up first, so that
# a change below reaches it too.
execute as @e[type=item_display,tag=arnis.xp] unless score @s arnis.xs = #epoch arnis.xing run function arnis:xing/catch_up with entity @s item.components."minecraft:custom_data"
# Where every train is by its timetable, and so what each crossing shows now.
function arnis:xing/check
function arnis:xing/flash
